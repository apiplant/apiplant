//! The start-up banner.
//!
//! Everything else the server says on boot goes through `tracing`, one line per
//! thing it loaded. The banner is the exception: it is the *address book* the
//! user needs the moment the server is up — where the API is, where the docs
//! are, where the dashboard is — so it is printed to stdout as a block, after
//! the log lines, rather than buried among them.
//!
//! The drawing itself lives in [`crate::term`], which every other command
//! prints through too.
//!
//! When `[server] domain` names one or more hosts the server only answers those
//! hosts, so there is a box per domain, each with that domain's own links. With
//! no domain configured the server answers for any host and there is one box,
//! titled with the address it is bound to.

use crate::term;

/// What the banner needs to know about the app it is announcing.
pub(crate) struct Banner {
    /// The app's display name, printed above the boxes.
    pub name: String,
    /// `http` or `https`.
    pub scheme: &'static str,
    /// The bound `host:port`, used when no domain is configured.
    pub addr: String,
    /// The API mount point, e.g. `/api` (may be empty for the root).
    pub base_path: String,
    /// The docs path relative to `base_path`, when docs are enabled.
    pub docs_path: Option<String>,
    /// The dashboard path, e.g. `/admin`, when it is being served.
    pub admin_path: Option<String>,
    /// Whether the app's `public/` site is served at the root.
    pub site: bool,
    /// Hosts the server answers for, empty when it answers for any.
    pub domains: Vec<String>,
}

impl Banner {
    /// Print the wordmark, the app's name, and one box per host.
    pub(crate) fn print(&self) {
        term::wordmark();
        term::heading(&self.name, None);

        // With no domain configured the server answers for any host, so the
        // single box is titled with the address it is bound to instead.
        if self.domains.is_empty() {
            term::links(&self.addr, &self.links(&self.addr));
        } else {
            for domain in &self.domains {
                term::links(domain, &self.links(domain));
            }
        }
        println!();
    }

    /// The labelled links for one host, in the order they are printed.
    fn links(&self, host: &str) -> Vec<(&'static str, String)> {
        let base = format!("{}://{host}{}", self.scheme, self.base_path);
        let mut links = vec![("API", base.clone())];
        if let Some(docs) = &self.docs_path {
            links.push(("Docs", format!("{base}{docs}")));
        }
        if let Some(admin) = &self.admin_path {
            links.push(("Admin", format!("{}://{host}{admin}/", self.scheme)));
        }
        if self.site {
            links.push(("Site", format!("{}://{host}/", self.scheme)));
        }
        links
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banner() -> Banner {
        Banner {
            name: "Acme API".into(),
            scheme: "http",
            addr: "0.0.0.0:8099".into(),
            base_path: "/api".into(),
            docs_path: Some("/docs".into()),
            admin_path: Some("/admin".into()),
            site: true,
            domains: vec![],
        }
    }

    #[test]
    fn links_are_built_from_the_host_and_the_mount_point() {
        let b = banner();
        let links = b.links("api.example.test");
        assert_eq!(
            links,
            vec![
                ("API", "http://api.example.test/api".to_string()),
                ("Docs", "http://api.example.test/api/docs".to_string()),
                ("Admin", "http://api.example.test/admin/".to_string()),
                ("Site", "http://api.example.test/".to_string()),
            ]
        );
    }

    #[test]
    fn disabled_docs_and_admin_are_left_out() {
        let mut b = banner();
        b.docs_path = None;
        b.admin_path = None;
        b.site = false;
        let links = b.links("localhost:8099");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0, "API");
    }

    #[test]
    fn printing_a_domain_box_does_not_panic_on_a_short_domain() {
        let mut b = banner();
        b.domains = vec!["a.io".into()];
        b.print();
    }
}
