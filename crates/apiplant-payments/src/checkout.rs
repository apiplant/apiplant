//! Starting a purchase, and managing one that has already started.
//!
//! Everything here hands the hard parts to Stripe on purpose. The checkout is
//! Stripe's hosted page and the self-service screens are Stripe's portal,
//! which means card details, 3-D Secure, wallets, tax numbers, dunning and the
//! whole of PCI scope happen on a domain that is not the app's. What comes
//! back is a URL to redirect somebody to — that is the entire integration
//! surface, and it is small because everything expensive is on the other side
//! of it.

use std::str::FromStr;

use stripe_billing::billing_portal_session::CreateBillingPortalSession;
use stripe_billing::subscription::{CancelSubscription, RetrieveSubscription, UpdateSubscription};
use stripe_checkout::checkout_session::{
    CreateCheckoutSession, CreateCheckoutSessionAutomaticTax, CreateCheckoutSessionCustomerUpdate,
    CreateCheckoutSessionCustomerUpdateAddress, CreateCheckoutSessionCustomerUpdateName,
    CreateCheckoutSessionCustomerUpdateShipping, CreateCheckoutSessionLineItems,
    CreateCheckoutSessionPaymentIntentData, CreateCheckoutSessionShippingAddressCollection,
    CreateCheckoutSessionShippingAddressCollectionAllowedCountries,
    CreateCheckoutSessionSubscriptionData, CreateCheckoutSessionTaxIdCollection,
};
use stripe_shared::{
    CheckoutSessionBillingAddressCollection, CheckoutSessionMode, CustomerId, Subscription,
    SubscriptionId,
};

use crate::catalog::{nonempty, org_metadata};
use crate::types::{StartedCheckout, SubscriptionState};
use crate::{CheckoutSpec, Payments, PaymentsError, ORG_METADATA_KEY, PRICE_METADATA_KEY};

impl Payments {
    /// Start a checkout, and get the URL to send the buyer to.
    ///
    /// The `recurring` flag on the spec is what decides between the two modes
    /// — a subscription or a single payment — because it is a property of the
    /// price, and asking the caller to restate it as a mode is asking them to
    /// get it wrong.
    ///
    /// The session carries the organisation and the app's own price id as
    /// metadata. That is not decoration: the webhook that arrives minutes
    /// later has no session, no headers and no caller, and this is the only
    /// thread back to a tenant.
    pub async fn checkout(&self, spec: CheckoutSpec) -> Result<StartedCheckout, PaymentsError> {
        let price = nonempty(&spec.stripe_price_id)
            .ok_or_else(|| PaymentsError::Request("a checkout needs a price".into()))?
            .to_string();

        // The customer is resolved before the session so that the id can be
        // stored against the organisation whatever the buyer then does: a
        // person who abandons the page has still been created in Stripe, and
        // creating them again on the next attempt is how an org ends up with
        // two customers.
        let customer = if let Some(id) = nonempty(&spec.stripe_customer_id) {
            id.to_string()
        } else {
            let mut wanted = spec.customer.clone();
            if wanted.organization_id.is_empty() {
                wanted.organization_id = spec.organization_id.clone();
            }
            self.ensure_customer(&wanted).await?.stripe_customer_id
        };
        let customer_id = CustomerId::from_str(&customer)
            .map_err(|e| PaymentsError::Request(format!("customer id {customer:?}: {e}")))?;

        let mode = match spec.recurring {
            true => CheckoutSessionMode::Subscription,
            false => CheckoutSessionMode::Payment,
        };
        let success_url = self.return_url(&spec.success_url, &self.config.success_url, "success");
        let cancel_url = self.return_url(&spec.cancel_url, &self.config.cancel_url, "cancelled");

        let mut metadata = org_metadata(&spec.organization_id);
        if let Some(price_row) = nonempty(&spec.price_id) {
            metadata.insert(PRICE_METADATA_KEY.to_string(), price_row.to_string());
        }

        let mut line_item = CreateCheckoutSessionLineItems::new();
        line_item.price = Some(price);
        line_item.quantity = Some(spec.quantity.max(1));

        let mut params = CreateCheckoutSession::new()
            .mode(mode)
            .customer(customer_id.clone())
            .success_url(&success_url)
            .cancel_url(&cancel_url)
            .line_items(vec![line_item]);
        // Empty metadata is refused by Stripe as an unset attempt, so an
        // organisation-less checkout sends none rather than a blank.
        if !metadata.is_empty() {
            params = params.metadata(metadata.clone());
        }
        if spec.allow_promotion_codes {
            params = params.allow_promotion_codes(true);
        }

        // Let Stripe Tax work out what the buyer owes, and collect enough of
        // an address for it to be able to. With no registrations configured
        // in Stripe this adds nothing and costs nothing.
        //
        // The `customer_update` is not optional decoration. Automatic tax
        // needs an address *on the customer*, and a session that names an
        // existing customer is refused outright unless it is allowed to save
        // the one the buyer types back onto them. Since a customer is always
        // named here — that is how a second purchase reaches the same invoices
        // — leaving this out means every checkout in an app with tax on fails,
        // which is a thing that only shows up against a real Stripe.
        if self.config.automatic_tax {
            let mut customer_update = CreateCheckoutSessionCustomerUpdate::new();
            customer_update.address = Some(CreateCheckoutSessionCustomerUpdateAddress::Auto);
            customer_update.name = Some(CreateCheckoutSessionCustomerUpdateName::Auto);
            // Only when the page actually asks for one: Stripe refuses
            // `shipping: auto` on a session that collects no shipping
            // address, so this cannot simply be on.
            customer_update.shipping = spec
                .shipping
                .then_some(CreateCheckoutSessionCustomerUpdateShipping::Auto);

            params = params
                .automatic_tax(CreateCheckoutSessionAutomaticTax::new(true))
                .customer_update(customer_update);
        }
        if self.config.collects_tax_ids() {
            params = params.tax_id_collection(CreateCheckoutSessionTaxIdCollection::new(true));
        }
        params = params.billing_address_collection(
            match self
                .config
                .billing_address
                .trim()
                .to_ascii_lowercase()
                .as_str()
            {
                "required" | "always" => CheckoutSessionBillingAddressCollection::Required,
                _ => CheckoutSessionBillingAddressCollection::Auto,
            },
        );

        // Something has to be posted, so ask where. The list of countries is
        // configuration rather than a per-call argument because "where we
        // ship to" is a fact about the business, and a checkout that quietly
        // accepted an address outside it would be a promise the warehouse
        // cannot keep.
        if spec.shipping {
            let countries = self.config.shipping_destinations();
            if countries.is_empty() {
                return Err(PaymentsError::Request(
                    "this product is shippable, but [payments] shipping_countries is empty — \
                     there is nowhere to send it"
                        .into(),
                ));
            }
            params = params.shipping_address_collection(
                CreateCheckoutSessionShippingAddressCollection::new(parse_countries(&countries)?),
            );
        }

        // The subscription itself carries the metadata too. Subscription
        // events (`customer.subscription.updated`, a renewal two years from
        // now) reference the subscription and know nothing of the session that
        // created it, so without this copy they arrive unattributable.
        if spec.recurring {
            let mut data = CreateCheckoutSessionSubscriptionData::new();
            data.metadata = (!metadata.is_empty()).then_some(metadata);
            if spec.trial_days > 0 {
                data.trial_period_days = Some(spec.trial_days);
            }
            params = params.subscription_data(data);
        } else {
            // And a one-off's payment intent carries it for the same reason:
            // `payment_intent.succeeded` is what reports the purchase, and it
            // arrives referring to nothing but itself. Without this copy the
            // charge is an amount with no tenant and no idea what was bought —
            // and, since an invoice's own intents carry no such metadata, it
            // is also how the two are told apart.
            let mut data = CreateCheckoutSessionPaymentIntentData::new();
            data.metadata = (!metadata.is_empty()).then_some(metadata);
            params = params.payment_intent_data(data);
        }

        let session = self
            .call("starting the checkout", params.send(&self.client))
            .await?;

        let url = session.url.clone().ok_or_else(|| {
            // Only happens for a session in a UI mode that has no hosted page,
            // which is not one we ask for — but a `None` here would otherwise
            // become a redirect to nowhere.
            PaymentsError::Provider("the checkout session came back with no URL".into())
        })?;

        Ok(StartedCheckout {
            session_id: session.id.to_string(),
            url,
            stripe_customer_id: customer,
            mode: match spec.recurring {
                true => "subscription".into(),
                false => "payment".into(),
            },
        })
    }

    /// A link to Stripe's customer portal, where somebody changes their card,
    /// downloads invoices, updates their tax number or cancels — all of it
    /// without the app implementing any of it, and all of it landing back here
    /// as webhooks.
    pub async fn portal(
        &self,
        stripe_customer_id: &str,
        return_url: &str,
    ) -> Result<String, PaymentsError> {
        let customer = nonempty(stripe_customer_id).ok_or_else(|| {
            PaymentsError::Request("the portal needs the customer whose billing it is".into())
        })?;
        let id = CustomerId::from_str(customer)
            .map_err(|e| PaymentsError::Request(format!("customer id {customer:?}: {e}")))?;

        let back = self.return_url(return_url, &self.config.portal_return_url, "");
        let session = self
            .call(
                "opening the billing portal",
                CreateBillingPortalSession::new()
                    .customer(id)
                    .return_url(&back)
                    .send(&self.client),
            )
            .await?;
        Ok(session.url)
    }

    /// A subscription's current state, straight from Stripe.
    ///
    /// The `billing_subscription` table is a copy kept by the webhook, and
    /// reading it is nearly always the right thing. This is for the moment it
    /// is not: an entitlement decision worth a round trip, or a support
    /// question about a row that looks wrong.
    pub async fn subscription(&self, id: &str) -> Result<SubscriptionState, PaymentsError> {
        let id = SubscriptionId::from_str(id.trim())
            .map_err(|e| PaymentsError::Request(format!("subscription id {id:?}: {e}")))?;
        let subscription = self
            .call(
                "retrieving the subscription",
                RetrieveSubscription::new(id).send(&self.client),
            )
            .await?;
        Ok(subscription_state(&subscription))
    }

    /// Cancel a subscription.
    ///
    /// `at_period_end` — the default everywhere this is reachable from — keeps
    /// the customer subscribed until the period they have already paid for
    /// runs out. Cancelling immediately takes away time somebody bought and
    /// refunds nothing, so it is available but never the default.
    pub async fn cancel_subscription(
        &self,
        id: &str,
        at_period_end: bool,
    ) -> Result<SubscriptionState, PaymentsError> {
        let id = SubscriptionId::from_str(id.trim())
            .map_err(|e| PaymentsError::Request(format!("subscription id {id:?}: {e}")))?;

        let subscription = if at_period_end {
            self.call(
                "cancelling the subscription",
                UpdateSubscription::new(id)
                    .cancel_at_period_end(true)
                    .send(&self.client),
            )
            .await?
        } else {
            // Stripe's DELETE ends it now and answers with the ended
            // subscription, which is the state we want to report either way.
            self.call(
                "ending the subscription",
                CancelSubscription::new(id).send(&self.client),
            )
            .await?
        };
        Ok(subscription_state(&subscription))
    }

    /// Where Stripe sends somebody back to.
    ///
    /// Three sources, most specific first: what the call asked for, what
    /// `[payments]` configured, and — failing both — the dashboard's billing
    /// screen, which exists in every app that hasn't switched the dashboard
    /// off and is a better landing place than an error.
    fn return_url(&self, requested: &str, configured: &str, outcome: &str) -> String {
        if let Some(url) = nonempty(requested) {
            return url.to_string();
        }
        if let Some(url) = nonempty(configured) {
            return url.to_string();
        }
        match nonempty(outcome) {
            Some(outcome) => format!("{}?checkout={outcome}", self.fallback_base),
            None => self.fallback_base.clone(),
        }
    }
}

/// Turn configured country codes into the ones Stripe's client accepts.
///
/// A typo is refused here rather than sent: Stripe would reject the whole
/// session, and "GB, DE, XX" failing as *one* unusable code is a better error
/// than a checkout that stopped working after somebody edited a list.
fn parse_countries(
    codes: &[String],
) -> Result<Vec<CreateCheckoutSessionShippingAddressCollectionAllowedCountries>, PaymentsError> {
    codes
        .iter()
        .map(|code| {
            // Parsing never fails — an unrecognised code becomes `Unknown`,
            // which Stripe's own client documents as not fit to send. Treating
            // it as the error it is keeps a typo a named configuration
            // problem rather than a rejected session.
            match CreateCheckoutSessionShippingAddressCollectionAllowedCountries::from_str(code) {
                Ok(CreateCheckoutSessionShippingAddressCollectionAllowedCountries::Unknown(_))
                | Err(_) => Err(PaymentsError::Config(format!(
                    "[payments] shipping_countries: {code:?} is not a country Stripe ships to"
                ))),
                Ok(country) => Ok(country),
            }
        })
        .collect()
}

/// Read a Stripe subscription into the app's shape.
pub(crate) fn subscription_state(subscription: &Subscription) -> SubscriptionState {
    let item = subscription.items.data.first();
    SubscriptionState {
        stripe_subscription_id: subscription.id.to_string(),
        stripe_customer_id: subscription.customer.id().to_string(),
        stripe_price_id: item
            .map(|item| item.price.id.to_string())
            .unwrap_or_default(),
        status: subscription.status.as_str().to_string(),
        quantity: item.and_then(|item| item.quantity).unwrap_or(1) as i64,
        organization_id: subscription
            .metadata
            .get(ORG_METADATA_KEY)
            .cloned()
            .unwrap_or_default(),
        // The billing period belongs to the item, not the subscription: one
        // subscription can hold items on different cycles, so there is no
        // single period to ask the parent for.
        current_period_end: item.map(|item| item.current_period_end),
        trial_end: subscription.trial_end,
        canceled_at: subscription.canceled_at,
        cancel_at_period_end: subscription.cancel_at_period_end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::PaymentsConfig;

    fn payments(config: PaymentsConfig) -> Payments {
        Payments::from_config(
            &PaymentsConfig {
                provider: "stripe".into(),
                secret_key: "sk_test_abc".into(),
                ..config
            },
            "https://example.com/admin/#/billing",
        )
        .unwrap()
        .unwrap()
    }

    /// The list is configuration, so a typo in it must fail as a named bad
    /// code rather than as a whole session Stripe rejects for reasons of its
    /// own.
    #[test]
    fn shipping_countries_are_checked_before_they_are_sent() {
        let good = parse_countries(&["GB".into(), "DE".into(), "US".into()]).unwrap();
        assert_eq!(good.len(), 3);

        let error = parse_countries(&["GB".into(), "XX".into()]).unwrap_err();
        assert!(error.to_string().contains("XX"), "{error}");
    }

    /// A shippable product in an app that has said nowhere to ship to is a
    /// misconfiguration, and taking the money first would mean an order with
    /// no address on it.
    #[test]
    fn shipping_with_no_destinations_configured_is_refused() {
        let digital_only = payments(PaymentsConfig::default());
        assert!(digital_only.config.shipping_destinations().is_empty());
        assert!(!digital_only.config.ships());
    }

    /// Codes are normalised on the way out — a config file saying `gb` and one
    /// saying `GB` are the same list, and the duplicate is not sent twice.
    #[test]
    fn destinations_are_upper_cased_and_deduplicated() {
        let shop = payments(PaymentsConfig {
            shipping_countries: vec!["gb".into(), "GB".into(), " de ".into(), "".into()],
            ..PaymentsConfig::default()
        });
        assert_eq!(shop.config.shipping_destinations(), ["GB", "DE"]);
        assert!(shop.config.ships());
    }

    /// The fallback differs by kind on purpose: filing a posted mug under the
    /// digital code is how automatic tax returns a confident wrong number.
    #[test]
    fn the_default_tax_code_follows_whether_the_thing_is_posted() {
        let shop = payments(PaymentsConfig::default());
        assert_eq!(shop.config.default_tax_code(true), "txcd_99999999");
        assert_eq!(shop.config.default_tax_code(false), "txcd_10000000");

        // A product naming its own code keeps it, whichever kind it is.
        let spec = crate::ProductSpec {
            tax_code: "txcd_10502000".into(),
            shippable: true,
            ..Default::default()
        };
        assert_eq!(shop.tax_code(&spec).as_deref(), Some("txcd_10502000"));

        // Blanked in config means "send nothing and let Stripe decide".
        let silent = payments(PaymentsConfig {
            digital_tax_code: String::new(),
            ..PaymentsConfig::default()
        });
        assert_eq!(silent.tax_code(&crate::ProductSpec::default()), None);
    }

    #[test]
    fn a_return_url_prefers_the_call_then_the_config_then_the_dashboard() {
        let configured = payments(PaymentsConfig {
            success_url: "https://app.example.com/thanks".into(),
            ..PaymentsConfig::default()
        });
        assert_eq!(
            configured.return_url("https://per-call.example.com", "https://config", "success"),
            "https://per-call.example.com"
        );
        assert_eq!(
            configured.return_url("", &configured.config.success_url.clone(), "success"),
            "https://app.example.com/thanks"
        );

        // With neither, the buyer lands somewhere real and is told which of
        // the two things happened.
        let bare = payments(PaymentsConfig::default());
        assert!(bare
            .return_url("", "", "success")
            .ends_with("?checkout=success"));
        assert!(!bare.return_url("", "", "").contains('?'));
    }
}
