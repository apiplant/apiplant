/**
 * One publisher and three subscribers, to show both halves of a queue.
 *
 *   POST /api/functions/checkout   authenticated  - marks an order paid, publishes
 *   fulfilOrder                    subscriber     - the slow work, done afterwards
 *   notifyOps                      subscriber     - a second handler, same topic
 *   releaseStock                   subscriber     - handles the model's [publish]
 *
 * The thing to notice is what `checkout` does *not* do: it does not send the
 * receipt, does not tell the warehouse, and does not wait for either. It writes
 * one row, publishes one message, and returns. Everything slow, everything that
 * talks to somebody else's server, and everything that is allowed to fail
 * happens on the other side of `queue.publish`.
 */

import { BadRequest, db, defineFunctions, delivery, log, queue, s, sql } from "apiplant";

export default defineFunctions({
  checkout: {
    version: "1.0.0",
    description: "Marks an order paid and queues the work that follows.",
    permission: "authenticated",
    method: "POST",
    input: s.object({
      reference: s.string({ minLength: 1, description: "The order's reference." }),
    }),
    output: s.object({
      reference: s.string(),
      queued: s.integer({ description: "How many subscribers the message went to." }),
    }),

    handler(input) {
      const order = db.first<{ id: string; status: string }>(
        sql`SELECT id, status FROM apiplant_order WHERE reference = ${input.reference}`,
      );
      if (!order) throw new BadRequest(`no order with reference \`${input.reference}\``);
      if (order.status === "paid") throw new BadRequest("that order is already paid");

      db.execute(sql`UPDATE apiplant_order SET status = 'paid' WHERE id = ${order.id}::uuid`);

      // Returns once the message is *committed*, not once it has been handled.
      // If the server is killed right here, the row is already in
      // `queue_message` and the handlers run when it comes back up.
      const published = queue.publish("order.paid", {
        order_id: order.id,
        reference: input.reference,
      });

      // `delivered: 0` would mean nothing subscribes to this topic. Not an
      // error — the message is still recorded — but nearly always a typo, so
      // it is worth saying so rather than wondering later.
      if (published.delivered === 0) {
        log.warn("order.paid has no subscribers; check [queues.subscribe]");
      }

      return { reference: input.reference, queued: published.delivered };
    },
  },

  fulfilOrder: {
    version: "1.0.0",
    description: "Fulfils a paid order. Runs from the queue, not from a request.",

    /**
     * A subscriber is an ordinary function. The message body arrives as its
     * ordinary input — so this can be POSTed to by hand to test it — and the
     * envelope (topic, id, which attempt) comes from `delivery()`.
     */
    handler(input: { order_id: string; reference: string }) {
      const message = delivery();

      /**
       * Delivery is at-least-once. A handler that succeeded and then died
       * before its row could be marked runs again, so anything above the first
       * attempt may be finishing work that already partly happened.
       *
       * The fix is not to detect the retry — it is to write the update so that
       * running it twice is running it once. `WHERE fulfilled_at IS NULL` does
       * exactly that, and costs nothing.
       */
      if (message && message.attempts > 1) {
        log.warn(`retrying ${message.topic} (attempt ${message.attempts})`);
      }

      const updated = db.execute(
        sql`UPDATE apiplant_order
               SET status = 'fulfilled', fulfilled_at = now()
             WHERE id = ${input.order_id}::uuid
               AND fulfilled_at IS NULL`,
      );

      log.info(`fulfilled ${input.reference} (${updated} row(s) changed)`);
      return { fulfilled: updated > 0 };
    },
  },

  notifyOps: {
    version: "1.0.0",
    description: "A second subscriber on the same topic, with its own retries.",

    /**
     * Both handlers get their own row in `queue_message`, so this one failing
     * never re-runs `fulfilOrder`. Throwing here is the honest thing to do: the
     * message goes back to `pending`, is retried on the backoff, and after
     * `max_attempts` is left `failed` for somebody to look at.
     *
     * Nothing about that reaches the person who checked out. Their request
     * finished before this ever ran.
     */
    handler(input: { reference: string }) {
      log.info(`ops: order ${input.reference} is paid`);
      return { notified: true };
    },
  },

  releaseStock: {
    version: "1.0.0",
    description: "Handles the `order.cancelled` the order model announces.",

    /**
     * Nothing published this: the `[publish] after_delete` line in
     * `models/order.toml` did, and the message is the deleted row itself. The
     * DELETE returned 204 without waiting for any of this.
     */
    handler(input: { reference?: string; total_cents?: number }) {
      log.info(`releasing stock for cancelled order ${input.reference ?? "?"}`);
      return { released: true };
    },
  },
});
