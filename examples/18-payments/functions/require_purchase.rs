//! The other kind of paywall: bought outright, once.
//!
//! `require_plan` next door asks "are they subscribed *right now*", which is a
//! question with an expiry date on it. This one asks "did they ever pay for
//! this", which is a question that never stops being true — and the two are
//! genuinely different, which is why they are two functions and not one with a
//! flag.
//!
//! A one-off payment leaves a `billing_payment` row with `subscription_id`
//! null: that null is what distinguishes "they bought this" from "this month's
//! instalment of a subscription", and it is the whole query. Nothing here
//! talks to Stripe, for the same reason `require_plan` doesn't — the webhook
//! already wrote down what happened, and an entitlement check that costs a
//! round trip over the internet is an entitlement check that fails when
//! Stripe has a bad afternoon.

use apiplant_function::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

/// Config from `functions/require_purchase.toml`.
#[derive(Deserialize)]
#[serde(default)]
struct Settings {
    /// Payment statuses that count as bought.
    ///
    /// Only `succeeded` by default. A `pending` payment is a bank that has not
    /// said yes yet, and handing over a downloadable file on the strength of
    /// one is handing it over for free if the answer turns out to be no.
    paid_statuses: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            paid_statuses: vec!["succeeded".into()],
        }
    }
}

/// Whether this organisation has ever paid, outright, for the product behind
/// the given price.
///
/// The join runs payment → price → product rather than matching on the price
/// id directly, and that is deliberate: changing an amount in Stripe mints a
/// *new* price and archives the old one, so somebody who bought the ebook at
/// last year's price holds a `price_id` that no longer exists on the shelf.
/// Matching the product is what stops a price rise from repossessing what
/// people already own.
///
/// "Outright" is two conditions, not one. `subscription_id IS NULL` says this
/// charge was not an instalment of anything; `interval = ''` says the price
/// itself was a one-off. Either alone can be wrong — a subscription's first
/// invoice can arrive before the subscription row it belongs to, and would
/// look unattached for exactly as long as that takes.
fn has_bought(ctx: &Context<Settings>, organization: &str, product: &str) -> Result<bool, String> {
    let statuses: Vec<Value> = ctx
        .config()
        .paid_statuses
        .iter()
        .map(|status| json!(status))
        .collect();

    let row = ctx.query_one(
        "SELECT 1 AS found \
         FROM apiplant_billing_payment pay \
         JOIN apiplant_billing_price pr ON pr.id = pay.price_id \
         JOIN apiplant_billing_product prod ON prod.id = pr.product_id \
         WHERE pay.organization_id = $1::uuid \
           AND prod.name = $2 \
           AND pay.status IN (SELECT jsonb_array_elements_text($3::jsonb)) \
           AND pay.subscription_id IS NULL \
           AND coalesce(pr.interval, '') = '' \
         LIMIT 1",
        &[json!(organization), json!(product), Value::Array(statuses)],
    )?;
    Ok(row.is_some())
}

/// `before_create` on `download` — you may fetch what you have paid for.
///
/// 402 rather than 403 for the same reason as `require_plan`: "this costs
/// money and you have not paid" is a pricing page, and "you are not allowed"
/// is an apology. A client can tell them apart and show the right one.
fn require_purchase(ctx: &Context<Settings>, input: Value) -> Result<Value, String> {
    let hook = ctx.hook().ok_or("require_purchase is a lifecycle hook")?;

    let Some(organization) = hook.organization_id.clone() else {
        return Ok(reply::abort(
            400,
            "name the organization in the X-Organization header",
        ));
    };

    // The row names what is being fetched; the catalogue says what it costs.
    // An empty title is refused rather than treated as "anything", because a
    // paywall whose default is "let them through" is not a paywall.
    let product = input
        .get("product")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if product.is_empty() {
        return Ok(reply::abort(422, "`product` is required — what to download"));
    }

    if !has_bought(ctx, &organization, &product)? {
        return Ok(reply::abort(
            402,
            &format!("nobody here has bought {product:?}; see /api/billing_price"),
        ));
    }

    Ok(reply::replace(input))
}

apiplant_function::functions! {
    {
        name: "require_purchase",
        description: "Refuses a download unless the organisation has bought it outright.",
        method: Post,
        permission: "private",         // a hook, not an endpoint
        handler: require_purchase,
    },
}
