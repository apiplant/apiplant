//! The catalogue, and the customer it is sold to.
//!
//! These are the calls behind the `billing_product`, `billing_price` and
//! `billing_customer` resources: an admin edits a row, a hook lands here, and
//! Stripe ends up holding the same facts.
//!
//! The direction matters. Products and prices flow *out* — the app's tables
//! are the source and Stripe is the copy — while customers flow both ways,
//! because a buyer can change their own address and tax number in the Stripe
//! portal and the app finds out from a webhook.

use std::collections::HashMap;
use std::str::FromStr;

use stripe_core::customer::{
    CreateCustomer, RetrieveCustomer, RetrieveCustomerReturned, SearchCustomer, UpdateCustomer,
};
use stripe_product::price::{
    CreatePrice, CreatePriceRecurring, CreatePriceRecurringInterval, RetrievePrice, UpdatePrice,
};
use stripe_product::product::{CreateProduct, UpdateProduct};
use stripe_shared::{Customer, CustomerId, Price, PriceId, PriceTaxBehavior, ProductId};
use stripe_types::Currency;

use crate::types::{CustomerSpec, CustomerState, Interval, PriceSpec, TaxBehavior};
use crate::{Payments, PaymentsError, ORG_METADATA_KEY};

/// What happened to a price that was saved.
#[derive(Debug, Clone)]
pub struct PriceOutcome {
    /// The Stripe price that is now current.
    pub id: String,
    /// Whether this is a *new* Stripe price standing in for an old one, which
    /// is what an amount change costs. The caller writes the new id back to
    /// its row; anything already subscribed stays on the old price until it is
    /// moved deliberately.
    pub replaced: bool,
}

impl Payments {
    /// Find or create the Stripe customer for an organisation, and tell us
    /// what Stripe now holds about them.
    ///
    /// Three ways in, tried in order, because each of them is a real state an
    /// app arrives in:
    ///
    /// 1. a `stripe_customer_id` we already stored — used as given;
    /// 2. no id, but a Stripe customer whose metadata names this organisation
    ///    — adopted, which is what makes this safe to call twice after a row
    ///    was lost or an environment was restored from a backup;
    /// 3. neither — created.
    ///
    /// Never two. A second customer for one organisation splits its payment
    /// methods, its invoices and its tax status across two records that
    /// neither system will ever reconcile.
    pub async fn ensure_customer(
        &self,
        spec: &CustomerSpec,
    ) -> Result<CustomerState, PaymentsError> {
        if let Some(id) = nonempty(&spec.stripe_customer_id) {
            let id = CustomerId::from_str(id)
                .map_err(|e| PaymentsError::Request(format!("customer id {id:?}: {e}")))?;
            let found = self
                .call(
                    "retrieving the customer",
                    RetrieveCustomer::new(id).send(&self.client),
                )
                .await?;
            // Stripe answers "deleted" for a customer that has been removed
            // rather than 404-ing, and billing a deleted customer fails much
            // later with a message about nothing in particular.
            let RetrieveCustomerReturned::Customer(customer) = found else {
                return Err(PaymentsError::Provider(format!(
                    "the stripe customer {:?} has been deleted",
                    spec.stripe_customer_id
                )));
            };
            return Ok(customer_state(&customer));
        }

        if let Some(found) = self.find_customer_by_organization(spec).await? {
            return Ok(found);
        }

        let mut params = CreateCustomer::new();
        if let Some(email) = nonempty(&spec.email) {
            params = params.email(email);
        }
        if let Some(name) = nonempty(&spec.name) {
            params = params.name(name);
        }
        let metadata = org_metadata(&spec.organization_id);
        if !metadata.is_empty() {
            params = params.metadata(metadata);
        }
        let customer = self
            .call("creating the customer", params.send(&self.client))
            .await?;
        tracing::info!(
            customer = %customer.id,
            organization = %spec.organization_id,
            "created a stripe customer"
        );
        Ok(customer_state(&customer))
    }

    /// Look for a customer already carrying this organisation's metadata.
    ///
    /// Stripe's search index is eventually consistent — a customer created a
    /// second ago may not be findable — so this is a recovery path, not the
    /// primary one. It is what stops a restored backup from doubling every
    /// customer, and it is deliberately not what a normal checkout relies on.
    async fn find_customer_by_organization(
        &self,
        spec: &CustomerSpec,
    ) -> Result<Option<CustomerState>, PaymentsError> {
        let Some(org) = nonempty(&spec.organization_id) else {
            return Ok(None);
        };
        let query = format!("metadata['{ORG_METADATA_KEY}']:'{}'", escape_query(org));
        // A search that fails is not a reason to refuse the checkout: the
        // worst case is the customer we were about to create anyway.
        match SearchCustomer::new(query).limit(1).send(&self.client).await {
            Ok(found) => Ok(found.data.first().map(customer_state)),
            Err(error) => {
                tracing::debug!(%error, "customer search failed; creating a new customer");
                Ok(None)
            }
        }
    }

    /// Update what Stripe holds about a customer — the address and name on
    /// their invoices.
    pub async fn update_customer(
        &self,
        stripe_customer_id: &str,
        spec: &CustomerSpec,
    ) -> Result<CustomerState, PaymentsError> {
        let id = CustomerId::from_str(stripe_customer_id).map_err(|e| {
            PaymentsError::Request(format!("customer id {stripe_customer_id:?}: {e}"))
        })?;
        let mut params = UpdateCustomer::new(id);
        if let Some(email) = nonempty(&spec.email) {
            params = params.email(email);
        }
        if let Some(name) = nonempty(&spec.name) {
            params = params.name(name);
        }
        let metadata = org_metadata(&spec.organization_id);
        if !metadata.is_empty() {
            params = params.metadata(metadata);
        }
        let customer = self
            .call("updating the customer", params.send(&self.client))
            .await?;
        Ok(customer_state(&customer))
    }

    /// Create or update a product, and return the Stripe id it now has.
    ///
    /// Products are mutable in Stripe, so this really is an update: renaming a
    /// plan renames it, and archiving it (`active = false`) takes it off the
    /// price list without touching anything already sold.
    pub async fn upsert_product(&self, spec: &crate::ProductSpec) -> Result<String, PaymentsError> {
        let name = spec.name.trim();
        if name.is_empty() {
            return Err(PaymentsError::Request("a product needs a name".into()));
        }
        let metadata = product_metadata(spec);

        if let Some(existing) = nonempty(&spec.stripe_product_id) {
            let id = ProductId::from_str(existing)
                .map_err(|e| PaymentsError::Request(format!("product id {existing:?}: {e}")))?;
            let mut params = UpdateProduct::new(id)
                .name(name)
                .description(spec.description.trim().to_string())
                .active(spec.active)
                .shippable(spec.shippable);
            // An empty map serializes as `metadata=`, which Stripe reads as an
            // attempt to unset the field and refuses outright. "No metadata"
            // has to mean sending none.
            if !metadata.is_empty() {
                params = params.metadata(metadata);
            }
            if let Some(code) = self.tax_code(spec) {
                params = params.tax_code(code);
            }
            let product = self
                .call("updating the product", params.send(&self.client))
                .await?;
            return Ok(product.id.to_string());
        }

        let mut params = CreateProduct::new(name)
            .active(spec.active)
            .shippable(spec.shippable);
        if !metadata.is_empty() {
            params = params.metadata(metadata);
        }
        let description = spec.description.trim();
        if !description.is_empty() {
            params = params.description(description);
        }
        if let Some(code) = self.tax_code(spec) {
            params = params.tax_code(code);
        }
        let product = self
            .call("creating the product", params.send(&self.client))
            .await?;
        Ok(product.id.to_string())
    }

    /// The tax code to file a product under: what the row says, or the
    /// configured default for its kind.
    ///
    /// The fallback is per-kind rather than a single value because the whole
    /// point of the code is that a posted thing and a downloaded thing are
    /// taxed differently — one default for both would be a wrong answer for
    /// at least half the catalogue.
    pub(crate) fn tax_code(&self, spec: &crate::ProductSpec) -> Option<String> {
        let code = match nonempty(&spec.tax_code) {
            Some(code) => code.to_string(),
            None => self.config.default_tax_code(spec.shippable),
        };
        // Blanked deliberately in config: send nothing and let Stripe fall
        // back to the account's own default category.
        nonempty(&code).map(str::to_string)
    }

    /// Create or update a price, replacing it when the change is one Stripe
    /// won't apply in place.
    ///
    /// A Stripe price is immutable in everything that matters: amount,
    /// currency, interval, trial and tax behaviour are fixed the moment it
    /// exists. Only its nickname and whether it is active can change. So this
    /// does one of two things:
    ///
    /// * **an in-place update**, when nothing material moved — a rename, an
    ///   archive;
    /// * **a replacement**, when something did — mint a new price, archive the
    ///   old one, and hand back the new id.
    ///
    /// A replacement is not a migration: anything already subscribed to the
    /// old price keeps paying the old amount, because that is what they agreed
    /// to and changing it silently is the kind of thing that ends up in a
    /// newspaper. Moving them is a separate, deliberate act.
    pub async fn upsert_price(&self, spec: &PriceSpec) -> Result<PriceOutcome, PaymentsError> {
        let product = nonempty(&spec.stripe_product_id).ok_or_else(|| {
            PaymentsError::Request("a price needs the product it belongs to".into())
        })?;
        if spec.unit_amount < 0 {
            return Err(PaymentsError::Request(
                "a price cannot be negative — that is a refund, not a price".into(),
            ));
        }

        if let Some(existing) = nonempty(&spec.stripe_price_id) {
            let id = PriceId::from_str(existing)
                .map_err(|e| PaymentsError::Request(format!("price id {existing:?}: {e}")))?;
            let current = self
                .call(
                    "retrieving the price",
                    RetrievePrice::new(id.clone()).send(&self.client),
                )
                .await?;

            if !spec.differs_materially_from(&price_spec(&current)) {
                let mut params = UpdatePrice::new(id).active(spec.active);
                if let Some(nickname) = nonempty(&spec.nickname) {
                    params = params.nickname(nickname);
                }
                let updated = self
                    .call("updating the price", params.send(&self.client))
                    .await?;
                return Ok(PriceOutcome {
                    id: updated.id.to_string(),
                    replaced: false,
                });
            }

            let replacement = self.create_price(spec, product).await?;
            // Archive the old one *after* the new one exists: the other order
            // leaves a product with nothing buyable if the create fails.
            if let Err(error) = self
                .call(
                    "archiving the price",
                    UpdatePrice::new(id).active(false).send(&self.client),
                )
                .await
            {
                // The new price is live and the row will point at it; an
                // un-archived old one is untidy, not broken.
                tracing::warn!(%error, price = %existing, "could not archive the replaced price");
            }
            tracing::info!(
                old = %existing,
                new = %replacement,
                "price changed materially: created a replacement and archived the original"
            );
            return Ok(PriceOutcome {
                id: replacement,
                replaced: true,
            });
        }

        Ok(PriceOutcome {
            id: self.create_price(spec, product).await?,
            replaced: false,
        })
    }

    /// Mint a Stripe price from a spec.
    async fn create_price(&self, spec: &PriceSpec, product: &str) -> Result<String, PaymentsError> {
        let currency = self.currency(&spec.currency)?;
        let mut params = CreatePrice::new(currency)
            .product(product)
            .unit_amount(spec.unit_amount)
            .active(spec.active)
            .tax_behavior(match spec.tax_behavior {
                TaxBehavior::Inclusive => PriceTaxBehavior::Inclusive,
                TaxBehavior::Exclusive => PriceTaxBehavior::Exclusive,
                TaxBehavior::Unspecified => PriceTaxBehavior::Unspecified,
            });
        if let Some(nickname) = nonempty(&spec.nickname) {
            params = params.nickname(nickname);
        }

        if let Some(interval) = recurring_interval(spec.interval) {
            let mut recurring = CreatePriceRecurring::new(interval);
            recurring.interval_count = Some(spec.interval_count.max(1));
            recurring.trial_period_days = (spec.trial_days > 0).then_some(spec.trial_days);
            params = params.recurring(recurring);
        }

        let price = self
            .call("creating the price", params.send(&self.client))
            .await?;
        Ok(price.id.to_string())
    }

    /// The currency for an amount, falling back to `[payments] currency`.
    pub(crate) fn currency(&self, requested: &str) -> Result<Currency, PaymentsError> {
        let code = match nonempty(requested) {
            Some(code) => code.to_ascii_lowercase(),
            None => self.config.default_currency(),
        };
        // Parsing does not fail: an unrecognised code becomes `Unknown`, which
        // would be sent to Stripe and rejected there, on a price somebody is
        // trying to save. Catching it here keeps a typo a validation error
        // against the row that caused it.
        match Currency::from_str(&code) {
            Ok(Currency::Unknown(_)) | Err(_) => Err(PaymentsError::Request(format!(
                "{code:?} is not a currency Stripe takes"
            ))),
            Ok(currency) => Ok(currency),
        }
    }
}

/// Stripe's interval for one of ours; `None` for a one-off, which has no
/// `recurring` block at all.
fn recurring_interval(interval: Interval) -> Option<CreatePriceRecurringInterval> {
    match interval {
        Interval::OneOff => None,
        Interval::Day => Some(CreatePriceRecurringInterval::Day),
        Interval::Week => Some(CreatePriceRecurringInterval::Week),
        Interval::Month => Some(CreatePriceRecurringInterval::Month),
        Interval::Year => Some(CreatePriceRecurringInterval::Year),
    }
}

/// Read a Stripe price back into our own shape, so it can be compared with
/// what the app wants it to be.
fn price_spec(price: &Price) -> PriceSpec {
    let (interval, interval_count, trial_days) = match &price.recurring {
        Some(recurring) => (
            Interval::parse(recurring.interval.as_str()),
            recurring.interval_count,
            recurring.trial_period_days.unwrap_or(0),
        ),
        None => (Interval::OneOff, 1, 0),
    };
    PriceSpec {
        stripe_price_id: price.id.to_string(),
        stripe_product_id: String::new(),
        nickname: price.nickname.clone().unwrap_or_default(),
        unit_amount: price.unit_amount.unwrap_or(0),
        currency: price.currency.to_string().to_ascii_lowercase(),
        interval,
        interval_count,
        trial_days,
        tax_behavior: price
            .tax_behavior
            .as_ref()
            .map(|behavior| TaxBehavior::parse(behavior.as_str()))
            .unwrap_or(TaxBehavior::Unspecified),
        active: price.active,
    }
}

/// Read a Stripe customer into the app's shape, including the tax facts that
/// support gets asked about.
fn customer_state(customer: &Customer) -> CustomerState {
    let tax_id = customer
        .tax_ids
        .as_ref()
        .and_then(|list| list.data.first())
        .map(|id| id.value.clone())
        .unwrap_or_default();
    let tax_country = customer
        .address
        .as_ref()
        .and_then(|address| address.country.clone())
        .unwrap_or_default();
    CustomerState {
        stripe_customer_id: customer.id.to_string(),
        organization_id: customer
            .metadata
            .as_ref()
            .and_then(|m| m.get(ORG_METADATA_KEY).cloned())
            .unwrap_or_default(),
        email: customer.email.clone().unwrap_or_default(),
        name: customer.name.clone().unwrap_or_default(),
        tax_id,
        tax_country,
        // The fields this app actually reads, rather than the whole customer.
        // Stripe's response types deserialize with miniserde and cannot be
        // re-serialized; the full object arrives on the webhook path, which
        // has the raw JSON and overwrites this with it.
        details: serde_json::json!({
            "id": customer.id.to_string(),
            "email": customer.email,
            "name": customer.name,
            "address": customer.address.as_ref().map(|address| serde_json::json!({
                "country": address.country,
                "postal_code": address.postal_code,
                "city": address.city,
                "line1": address.line1,
                "line2": address.line2,
                "state": address.state,
            })),
        }),
    }
}

/// The metadata a Stripe object carries so a webhook can find its way home.
/// Empty when there is no organisation to name, rather than a blank entry.
pub(crate) fn org_metadata(organization_id: &str) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    if let Some(org) = nonempty(organization_id) {
        metadata.insert(ORG_METADATA_KEY.to_string(), org.to_string());
    }
    metadata
}

/// A product's own metadata: whatever the app put in `features`, flattened to
/// the string pairs Stripe stores.
fn product_metadata(spec: &crate::ProductSpec) -> HashMap<String, String> {
    spec.metadata
        .iter()
        .map(|(key, value)| {
            let text = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (key.clone(), text)
        })
        .collect()
}

/// Trim, and treat "" as absent — which is what an unset TOML field, an empty
/// column and a blank JSON string all arrive as.
pub(crate) fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

/// Escape a value going into a Stripe search query, whose string literals are
/// single-quoted.
fn escape_query(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::PaymentsConfig;

    fn payments(currency: &str) -> Payments {
        Payments::from_config(
            &PaymentsConfig {
                provider: "stripe".into(),
                secret_key: "sk_test_abc".into(),
                currency: currency.into(),
                ..PaymentsConfig::default()
            },
            "https://example.com/admin/#/billing",
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn an_amount_without_a_currency_takes_the_configured_one() {
        let payments = payments("EUR");
        assert_eq!(payments.currency("").unwrap(), Currency::EUR);
        assert_eq!(payments.currency("  ").unwrap(), Currency::EUR);
        // A price that names one still wins.
        assert_eq!(payments.currency("GBP").unwrap(), Currency::GBP);
    }

    #[test]
    fn a_currency_stripe_does_not_take_is_refused_by_name() {
        let error = payments("usd").currency("xyz").unwrap_err();
        assert!(error.to_string().contains("xyz"), "{error}");
    }

    #[test]
    fn a_one_off_price_has_no_recurring_block() {
        assert!(recurring_interval(Interval::OneOff).is_none());
        assert!(recurring_interval(Interval::Month).is_some());
    }

    #[test]
    fn organisation_metadata_is_omitted_rather_than_blank() {
        assert!(org_metadata("   ").is_empty());
        assert_eq!(
            org_metadata("org-1")
                .get(ORG_METADATA_KEY)
                .map(String::as_str),
            Some("org-1")
        );
    }

    /// A quote in an id would otherwise close the literal and change what the
    /// query matches.
    #[test]
    fn a_search_query_cannot_be_escaped_out_of() {
        assert_eq!(escape_query("o'brien"), "o\\'brien");
        assert_eq!(escape_query(r"back\slash"), r"back\\slash");
    }

    #[test]
    fn product_metadata_flattens_whatever_features_held() {
        let mut spec = crate::ProductSpec::default();
        spec.metadata
            .insert("tier".into(), serde_json::json!("pro"));
        spec.metadata.insert("seats".into(), serde_json::json!(25));
        let flat = product_metadata(&spec);
        // A JSON string keeps its characters rather than gaining quotes.
        assert_eq!(flat.get("tier").map(String::as_str), Some("pro"));
        assert_eq!(flat.get("seats").map(String::as_str), Some("25"));
    }
}
