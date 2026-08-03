//! Operator actions for the back office.
//!
//! Three things a person running this business needs to do that no CRUD form
//! covers: look at the numbers, correct a stock count, and clear out tickets
//! nobody has touched. Each one shows a different part of the action model:
//!
//! * `sales_summary` is `member` — the level `visibility` alone cannot express,
//!   and the right one for something every colleague may look at.
//! * `restock_variant` is `role:manager`, and its `admin` block asks for
//!   confirmation before it writes.
//! * `close_stale_tickets` is `role:admin` and hidden from anyone else's
//!   sidebar, because an action you cannot run is noise.
//!
//! The `Input` types are what make the dashboard render a real form: the
//! generated JSON Schema carries the field names, types and doc comments, and
//! the admin panel turns them into labelled inputs.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

// --- sales summary ---------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
struct SummaryInput {
    /// How many days back to count. Defaults to the last 30.
    #[serde(default = "thirty")]
    days: i64,
}

fn thirty() -> i64 {
    30
}

#[derive(Serialize, JsonSchema)]
struct SummaryOutput {
    /// Orders placed in the window.
    orders: i64,
    /// Their combined value, in cents.
    revenue_cents: i64,
    /// Orders still waiting to be confirmed.
    pending_orders: i64,
    /// Support tickets that are still open.
    open_tickets: i64,
}

fn sales_summary(ctx: &Context<()>, input: SummaryInput) -> Result<SummaryOutput, String> {
    let days = input.days.clamp(1, 365);
    let number = |row: Option<serde_json::Value>, key: &str| -> i64 {
        row.and_then(|row| row.get(key).and_then(serde_json::Value::as_i64))
            .unwrap_or(0)
    };

    let orders = ctx.query_one(
        "SELECT count(*)::int AS orders, \
                coalesce(sum(total_cents), 0)::bigint AS revenue \
         FROM apiplant_sales_order \
         WHERE created_at > now() - ($1::int * interval '1 day')",
        &[serde_json::json!(days)],
    )?;
    let revenue_cents = orders
        .as_ref()
        .and_then(|row| row.get("revenue").and_then(serde_json::Value::as_i64))
        .unwrap_or(0);

    let pending = ctx.query_one(
        "SELECT count(*)::int AS n FROM apiplant_sales_order WHERE status = 'pending'",
        &[],
    )?;
    let tickets = ctx.query_one(
        "SELECT count(*)::int AS n FROM apiplant_support_ticket WHERE status = 'open'",
        &[],
    )?;

    Ok(SummaryOutput {
        orders: number(orders, "orders"),
        revenue_cents,
        pending_orders: number(pending, "n"),
        open_tickets: number(tickets, "n"),
    })
}

// --- restock ---------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
struct RestockInput {
    /// The stock record to correct.
    inventory_level_id: String,
    /// The true number of units on the shelf.
    on_hand: i64,
    /// Why the count changed — kept in the log.
    #[serde(default)]
    reason: String,
}

#[derive(Serialize, JsonSchema)]
struct RestockOutput {
    /// Whether a stock record was found and updated.
    updated: bool,
    /// The count now recorded.
    on_hand: i64,
}

fn restock_variant(ctx: &Context<()>, input: RestockInput) -> Result<RestockOutput, String> {
    if input.on_hand < 0 {
        return Err("A stock count cannot be negative.".to_string());
    }
    let affected = ctx.execute(
        "UPDATE apiplant_inventory_level SET on_hand = $1, updated_at = now() WHERE id = $2::uuid",
        &[
            serde_json::json!(input.on_hand),
            serde_json::json!(input.inventory_level_id),
        ],
    )?;
    if affected == 0 {
        return Err("That stock record no longer exists.".to_string());
    }
    ctx.info(&format!(
        "stock for {} set to {} by {} ({})",
        input.inventory_level_id,
        input.on_hand,
        ctx.principal_id(),
        if input.reason.is_empty() {
            "no reason given"
        } else {
            &input.reason
        },
    ));
    Ok(RestockOutput {
        updated: true,
        on_hand: input.on_hand,
    })
}

// --- ticket housekeeping ---------------------------------------------------

#[derive(Deserialize, JsonSchema)]
struct StaleInput {
    /// Close resolved tickets untouched for at least this many days.
    #[serde(default = "fourteen")]
    older_than_days: i64,
}

fn fourteen() -> i64 {
    14
}

#[derive(Serialize, JsonSchema)]
struct StaleOutput {
    /// How many tickets were closed.
    closed: i64,
}

fn close_stale_tickets(ctx: &Context<()>, input: StaleInput) -> Result<StaleOutput, String> {
    let days = input.older_than_days.clamp(1, 3650);
    let closed = ctx.execute(
        "UPDATE apiplant_support_ticket \
         SET status = 'closed', updated_at = now() \
         WHERE status = 'resolved' \
           AND updated_at < now() - ($1::int * interval '1 day')",
        &[serde_json::json!(days)],
    )?;
    Ok(StaleOutput {
        closed: closed as i64,
    })
}

apiplant_function::functions! {
    {
        name: "sales_summary",
        description: "Orders, revenue and open tickets over a recent window.",
        method: Post,
        permission: "member",
        admin: {
            label: "Sales summary",
            group: "Reports",
            description: "A quick read on how trade is going.",
            run_label: "Show summary",
            order: 1,
        },
        handler: sales_summary,
    },
    {
        name: "restock_variant",
        description: "Corrects the recorded stock count for one warehouse line.",
        method: Post,
        permission: "role:manager",
        admin: {
            label: "Correct stock count",
            group: "Operations",
            description: "Use after a physical count disagrees with the system.",
            confirm: "This overwrites the recorded stock for that line. Continue?",
            run_label: "Update stock",
            order: 1,
        },
        handler: restock_variant,
    },
    {
        name: "close_stale_tickets",
        description: "Closes resolved support tickets nobody has touched in a while.",
        method: Post,
        permission: "role:admin",
        admin: {
            roles: ["admin"],
            label: "Close stale tickets",
            group: "Support",
            confirm: "Every resolved ticket older than the cut-off will be closed. Continue?",
            run_label: "Close tickets",
            order: 1,
        },
        handler: close_stale_tickets,
    },
}
