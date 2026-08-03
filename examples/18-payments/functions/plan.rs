//! The paywall.
//!
//! `require_plan` is a `before_create` hook on `document`: it refuses the write
//! when the organisation is not on a plan, which puts the paywall next to the
//! thing being paid for rather than in a middleware that has to guess.
//!
//! That is the only function in this example, and the absence of the others is
//! the point. *Reading* what the organisation is on is `GET
//! /api/billing_subscription` — org-scoped and permission-checked like every
//! other resource. *Changing* it is `POST /api/billing/portal`, which hands
//! the customer Stripe's own screens. A function re-exporting either would be
//! a worse version of a route that already exists.
//!
//! Nor does the paywall talk to Stripe. It is a query against
//! `billing_subscription`, which the webhook keeps current — that table is a
//! copy of Stripe's fact, kept locally precisely so that "may they do this"
//! costs a query and not a round trip over the internet on every request.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Config from `functions/require_plan.toml`.
#[derive(Deserialize)]
#[serde(default)]
struct Settings {
    /// Statuses that count as paying.
    ///
    /// `trialing` is in the default because a trial is a promise the app made.
    /// `past_due` is not: Stripe is still retrying the card, and continuing to
    /// serve through a dunning cycle is a business decision — which is exactly
    /// why it is a setting and not a hard-coded list.
    entitled_statuses: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            entitled_statuses: vec!["active".into(), "trialing".into()],
        }
    }
}

/// What the organisation is paying for, if anything.
#[derive(Serialize, JsonSchema)]
struct Plan {
    /// Whether they may use the paid features right now.
    entitled: bool,
    /// Stripe's own word for the state — `active`, `past_due`, `canceled`, …
    /// Empty when they have never subscribed.
    status: String,
    /// The product they are on, for showing on screen.
    product: String,
    /// Whatever the product's `features` column holds — seat limits, flags, a
    /// tier name. This is the app's own vocabulary, copied to Stripe as
    /// metadata so both systems say the same thing.
    features: Value,
    /// When the current period ends, as an RFC 3339 timestamp.
    renews_at: Option<String>,
}

/// Look up the organisation's live subscription and the product behind it.
///
/// One query across three tables, because the interesting answer is a join:
/// the subscription says *whether*, the product says *what*.
fn look_up(ctx: &Context<Settings>, organization: &str) -> Result<Plan, String> {
    let statuses: Vec<Value> = ctx
        .config()
        .entitled_statuses
        .iter()
        .map(|status| json!(status))
        .collect();

    // `= ANY($2)` rather than a built-up `IN (…)`: the list is configuration,
    // and configuration going into SQL by string concatenation is how a
    // settings file becomes an injection vector.
    let row = ctx.query_one(
        "SELECT s.status, \
                coalesce(p.name, '') AS product, \
                coalesce(p.features, 'null'::jsonb) AS features, \
                to_char(s.current_period_end, 'YYYY-MM-DD\"T\"HH24:MI:SSZ') AS renews_at \
         FROM billing_subscription s \
         LEFT JOIN billing_price pr ON pr.id = s.price_id \
         LEFT JOIN billing_product p ON p.id = pr.product_id \
         WHERE s.organization_id = $1::uuid \
         ORDER BY (s.status = ANY($2::text[])) DESC, s.created_at DESC \
         LIMIT 1",
        &[json!(organization), Value::Array(statuses)],
    )?;

    let Some(row) = row else {
        // Never subscribed. Not an error — it is the answer for most
        // organisations most of the time, and the front end wants to render it.
        return Ok(Plan {
            entitled: false,
            status: String::new(),
            product: String::new(),
            features: Value::Null,
            renews_at: None,
        });
    };

    let status = row["status"].as_str().unwrap_or_default().to_string();
    Ok(Plan {
        entitled: ctx.config().entitled_statuses.contains(&status),
        status,
        product: row["product"].as_str().unwrap_or_default().to_string(),
        features: row["features"].clone(),
        renews_at: row["renews_at"].as_str().map(str::to_string),
    })
}

/// `before_create` on `document` — the paywall.
///
/// Returning `abort` rejects the request and nothing is written. **402** is
/// the status for it: not 403, which says "you are not allowed", but "this
/// costs money and you have not paid" — a client can tell those apart and show
/// a pricing page for one and an apology for the other.
fn require_plan(ctx: &Context<Settings>, input: Value) -> Result<Value, String> {
    let hook = ctx.hook().ok_or("require_plan is a lifecycle hook")?;

    // Every org-scoped write names its organisation; a create that somehow
    // doesn't is refused rather than let through, because "no organisation"
    // must never read as "no subscription needed".
    let Some(organization) = hook.organization_id.clone() else {
        return Ok(reply::abort(
            400,
            "name the organization in the X-Organization header",
        ));
    };

    let plan = look_up(ctx, &organization)?;
    if !plan.entitled {
        let reason = match plan.status.as_str() {
            "" => "this organization is not on a plan".to_string(),
            "past_due" => "the last payment failed — update the card to carry on".to_string(),
            other => format!("this organization's subscription is {other}"),
        };
        return Ok(reply::abort(402, &format!("{reason}; see /api/billing_price")));
    }

    // Entitled. Returning the input unchanged is the hook saying "carry on" —
    // a `replace` that changed nothing would be the same thing said louder.
    Ok(reply::replace(input))
}

apiplant_function::functions! {
    {
        name: "require_plan",
        description: "Refuses a write unless the organisation is on a plan.",
        method: Post,
        permission: "private",         // a hook, not an endpoint
        handler: require_plan,
    },
}
