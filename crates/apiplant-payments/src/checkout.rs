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

use stripe::{
    BillingPortalSession, CheckoutSession, CheckoutSessionBillingAddressCollection,
    CheckoutSessionMode, CreateBillingPortalSession, CreateCheckoutSession,
    CreateCheckoutSessionAutomaticTax, CreateCheckoutSessionLineItems,
    CreateCheckoutSessionSubscriptionData, CreateCheckoutSessionTaxIdCollection, CustomerId,
    Subscription, SubscriptionId, UpdateSubscription,
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

        let mut params = CreateCheckoutSession::new();
        params.mode = Some(mode);
        params.customer = Some(customer_id.clone());
        params.success_url = Some(&success_url);
        params.cancel_url = Some(&cancel_url);
        params.line_items = Some(vec![CreateCheckoutSessionLineItems {
            price: Some(price),
            quantity: Some(spec.quantity.max(1)),
            ..Default::default()
        }]);
        params.metadata = Some(metadata.clone());
        if spec.allow_promotion_codes {
            params.allow_promotion_codes = Some(true);
        }

        // Let Stripe Tax work out what the buyer owes, and collect enough of
        // an address for it to be able to. With no registrations configured
        // in Stripe this adds nothing and costs nothing.
        if self.config.automatic_tax {
            params.automatic_tax = Some(CreateCheckoutSessionAutomaticTax {
                enabled: true,
                liability: None,
            });
        }
        if self.config.collects_tax_ids() {
            params.tax_id_collection = Some(CreateCheckoutSessionTaxIdCollection { enabled: true });
        }
        params.billing_address_collection = Some(
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

        // The subscription itself carries the metadata too. Subscription
        // events (`customer.subscription.updated`, a renewal two years from
        // now) reference the subscription and know nothing of the session that
        // created it, so without this copy they arrive unattributable.
        if spec.recurring {
            let mut data = CreateCheckoutSessionSubscriptionData::default();
            data.metadata = Some(metadata);
            if spec.trial_days > 0 {
                data.trial_period_days = Some(spec.trial_days);
            }
            params.subscription_data = Some(data);
        }

        let session = self
            .call(
                "starting the checkout",
                CheckoutSession::create(&self.client, params),
            )
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
        let mut params = CreateBillingPortalSession::new(id);
        params.return_url = Some(&back);

        let session = self
            .call(
                "opening the billing portal",
                BillingPortalSession::create(&self.client, params),
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
                Subscription::retrieve(&self.client, &id, &[]),
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
            let mut params = UpdateSubscription::new();
            params.cancel_at_period_end = Some(true);
            self.call(
                "cancelling the subscription",
                Subscription::update(&self.client, &id, params),
            )
            .await?
        } else {
            // Stripe's DELETE ends it now and answers with the ended
            // subscription, which is the state we want to report either way.
            self.call(
                "ending the subscription",
                Subscription::cancel(&self.client, &id, stripe::CancelSubscription::default()),
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

/// Read a Stripe subscription into the app's shape.
pub(crate) fn subscription_state(subscription: &Subscription) -> SubscriptionState {
    let item = subscription.items.data.first();
    SubscriptionState {
        stripe_subscription_id: subscription.id.to_string(),
        stripe_customer_id: subscription.customer.id().to_string(),
        stripe_price_id: item
            .and_then(|item| item.price.as_ref())
            .map(|price| price.id.to_string())
            .unwrap_or_default(),
        status: subscription.status.as_str().to_string(),
        quantity: item.and_then(|item| item.quantity).unwrap_or(1) as i64,
        organization_id: subscription
            .metadata
            .get(ORG_METADATA_KEY)
            .cloned()
            .unwrap_or_default(),
        current_period_end: Some(subscription.current_period_end),
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
