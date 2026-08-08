/*
 * The whole shop, in one file and no framework.
 *
 * There are three calls in it, and they are the three an app taking money
 * makes:
 *
 *   GET  /api/billing_product      what we sell        (public)
 *   GET  /api/billing_price        what it costs       (public)
 *   POST /api/billing/checkout     a URL to pay at     (an org admin)
 *
 * The interesting part is the gap between the second and the third. Reading the
 * price list needs nobody; buying needs an *administrator of an organisation*,
 * because a purchase commits a company's card. A shop cannot ask a first-time
 * visitor to go and register, come back, start a company and then try again —
 * so the buy dialog does all of it in one submit, and the checkout is the last
 * step rather than the first.
 */

const API = "/api";

// The signed-in buyer, kept for the length of the session only. A token in
// `localStorage` outlives the browser being closed on a shared machine, which
// for something that can charge a card is not a trade worth making.
const session = {
  get token() {
    return sessionStorage.getItem("token") || "";
  },
  get org() {
    return sessionStorage.getItem("org") || "";
  },
  get email() {
    return sessionStorage.getItem("email") || "";
  },
  save(token, org, email) {
    sessionStorage.setItem("token", token);
    sessionStorage.setItem("org", org);
    sessionStorage.setItem("email", email);
  },
  clear() {
    sessionStorage.clear();
  },
};

/** One JSON call, with the API's own error message preserved. */
async function api(path, { method = "GET", body, token, org } = {}) {
  const headers = { "Content-Type": "application/json" };
  if (token) headers["Authorization"] = `Bearer ${token}`;
  if (org) headers["X-Organization"] = org;

  const response = await fetch(API + path, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  });

  const text = await response.text();
  const data = text ? JSON.parse(text) : null;
  if (!response.ok) {
    // The API answers `{ "error": "…" }` and those messages are written to be
    // read by a person, so they are shown rather than replaced with "something
    // went wrong".
    throw new Error(data?.error || `${response.status} ${response.statusText}`);
  }
  return data;
}

/** An amount in the smallest unit, as money. `2900` in EUR is `€29.00`. */
function money(amount, currency) {
  return new Intl.NumberFormat(navigator.language, {
    style: "currency",
    // The API returns this upper-cased (the column forces the case), which is
    // also what `Intl` wants — so no normalising here.
    currency: currency || "EUR",
  }).format(amount / 100);
}

/** How a price is described on the button: what recurs, and how often. */
function cadence(price) {
  if (!price.interval) return "one-off";
  const every = price.interval_count > 1 ? `${price.interval_count} ` : "";
  return `per ${every}${price.interval}${price.interval_count > 1 ? "s" : ""}`;
}

// --- the catalogue ------------------------------------------------------

/**
 * Render the shop from the two public tables.
 *
 * A price belongs to a product, and it is the *product* that says whether the
 * thing is posted — so the card can say "we'll ask where to send it" without
 * the shop knowing anything about shipping. That fact lives in one column.
 */
async function showCatalogue() {
  const main = document.getElementById("catalogue");
  try {
    const [products, prices] = await Promise.all([
      api("/billing_product"),
      api("/billing_price"),
    ]);

    const live = prices.filter((price) => price.active !== false);
    const byProduct = new Map(products.map((product) => [product.id, product]));

    const cards = live
      .map((price) => card(price, byProduct.get(price.product_id)))
      .filter(Boolean);

    main.removeAttribute("aria-busy");
    main.innerHTML = cards.length
      ? cards.join("")
      : `<p class="muted">Nothing is on sale yet. Add a product and a price in
         the <a href="/admin/">dashboard</a>, or follow the example's README.</p>`;

    for (const button of main.querySelectorAll("button[data-price]")) {
      button.addEventListener("click", () => openBuy(button.dataset.price));
    }
  } catch (error) {
    main.removeAttribute("aria-busy");
    main.innerHTML = `<p class="error">Could not load the price list: ${escape(
      error.message
    )}</p>`;
  }
}

function card(price, product) {
  if (!product || product.active === false) return "";

  const kind = product.shippable
    ? `<span class="tag ships">Posted to you</span>`
    : price.interval
    ? `<span class="tag sub">Subscription</span>`
    : `<span class="tag download">Instant download</span>`;

  const trial = price.trial_days
    ? `<p class="trial">${price.trial_days} days free first</p>`
    : "";

  return `
    <article class="card">
      ${kind}
      <h2>${escape(product.name)}</h2>
      <p class="desc">${escape(product.description || "")}</p>
      <p class="price">
        <strong>${money(price.unit_amount, price.currency)}</strong>
        <span class="muted">${escape(cadence(price))}</span>
      </p>
      ${trial}
      <button data-price="${price.id}">
        ${price.interval ? "Subscribe" : "Buy"}
      </button>
      ${
        product.shippable
          ? `<p class="hint">We'll ask for a delivery address at the till.</p>`
          : ""
      }
    </article>`;
}

/** Text into HTML, so a product named by somebody else cannot write markup. */
function escape(value) {
  const div = document.createElement("div");
  div.textContent = value ?? "";
  return div.innerHTML;
}

// --- buying -------------------------------------------------------------

const dialog = document.getElementById("buy");
let chosenPrice = null;

async function openBuy(priceId) {
  chosenPrice = priceId;
  const error = document.getElementById("buy-error");
  error.hidden = true;

  // Somebody already signed in this session skips the form entirely: they have
  // an organisation, so there is nothing left to ask.
  if (session.token && session.org) {
    await goToStripe();
    return;
  }

  document.getElementById("buy-title").textContent = "Your details";
  document.getElementById("buy-summary").textContent =
    "Buying needs an account, so this is where you get one.";
  document.getElementById("email").value = session.email || "";
  dialog.showModal();
}

document.getElementById("buy-cancel").addEventListener("click", () => {
  dialog.close();
});

document.getElementById("buy-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const go = document.getElementById("buy-go");
  const error = document.getElementById("buy-error");
  error.hidden = true;
  go.disabled = true;
  go.textContent = "One moment…";

  try {
    const email = document.getElementById("email").value.trim();
    const password = document.getElementById("password").value;
    const company = document.getElementById("org").value.trim();

    await signIn(email, password, company);
    await goToStripe();
  } catch (failure) {
    error.textContent = failure.message;
    error.hidden = false;
    go.disabled = false;
    go.textContent = "Continue to payment";
  }
});

/**
 * Get the buyer an account and an organisation to buy on behalf of.
 *
 * Registration is tried first and a duplicate address falls back to signing in,
 * so one form serves a returning customer and a new one. The shop never has to
 * ask "do you already have an account?", which is a question the shop can
 * answer itself.
 */
async function signIn(email, password, company) {
  let auth;
  try {
    auth = await api("/auth/register", {
      method: "POST",
      body: { email, password },
    });
  } catch (failure) {
    // Already registered — the same details are then a sign-in. A wrong
    // password fails here, with the API's own message.
    auth = await api("/auth/login", {
      method: "POST",
      body: { email, password },
    });
  }

  const token = auth.token;

  // Buying takes `role:admin` of an organisation. Registering creates one —
  // named after the address, with the new account as its admin — so there is
  // normally nothing to do here but find it. A returning customer's is reused,
  // which is what keeps their invoices, their card and their subscriptions in
  // one place instead of spread across an organisation per purchase.
  const existing = await api("/organization", { token });
  let org =
    existing?.[0] ??
    (await api("/organization", {
      method: "POST",
      token,
      body: { name: company || email.split("@")[0] },
    }));

  // Put the name they typed on it, so the invoice says "Acme, Inc." rather
  // than the left half of an email address. Only when they gave one, and only
  // when it is actually different — a rename on every purchase would be a
  // write for nothing. Allowed because they administer it.
  if (company && company !== org.name) {
    try {
      org = await api(`/organization/${org.id}`, {
        method: "PATCH",
        token,
        org: org.id,
        body: { name: company },
      });
    } catch {
      // Not worth failing a sale over: the purchase is what the customer came
      // for, and the name on it can be fixed afterwards.
    }
  }

  session.save(token, org.id, email);
  showWho();
}

/**
 * Start the checkout and hand the browser over to Stripe.
 *
 * Everything expensive — the card form, 3-D Secure, wallets, the VAT number
 * box, the delivery address, the receipt — is on the other side of this
 * redirect, on Stripe's domain. The shop's entire payment integration is these
 * few lines.
 */
async function goToStripe() {
  const here = window.location.origin + window.location.pathname;
  const { url } = await api("/billing/checkout", {
    method: "POST",
    token: session.token,
    org: session.org,
    body: {
      price_id: chosenPrice,
      success_url: `${here}?checkout=success`,
      cancel_url: `${here}?checkout=cancelled`,
    },
  });
  window.location.href = url;
}

// --- coming back --------------------------------------------------------

function showOutcome() {
  const outcome = new URLSearchParams(window.location.search).get("checkout");
  if (!outcome) return;

  const box = document.getElementById("outcome");
  box.hidden = false;
  box.className = `notice ${outcome === "success" ? "good" : "warn"}`;
  box.innerHTML =
    outcome === "success"
      ? `<strong>Thank you.</strong> Your order is confirmed. The receipt is on
         its way, and what you bought is recorded against your account — by the
         webhook, not by this page, which is why closing the tab could not have
         lost it.`
      : `<strong>No charge was made.</strong> You left the payment page before
         it finished. Nothing was reserved and nothing was billed.`;

  // Take the parameter off the URL so a refresh is not a second "thank you".
  window.history.replaceState({}, "", window.location.pathname);
}

function showWho() {
  const who = document.getElementById("whoami");
  if (!session.email) {
    who.hidden = true;
    return;
  }
  who.hidden = false;
  who.innerHTML = `Signed in as <strong>${escape(session.email)}</strong> ·
    <a href="#" id="signout">sign out</a>`;
  document.getElementById("signout").addEventListener("click", (event) => {
    event.preventDefault();
    session.clear();
    showWho();
  });
}

showCatalogue();
showOutcome();
showWho();
