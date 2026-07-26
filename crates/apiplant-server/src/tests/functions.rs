//! Functions over HTTP: their routes, methods and access policies.

use super::*;

/// A function whose access policy is spelled with `permission` rather than
/// `visibility`, which is the only way to say `member`.
fn permissioned_function(name: &str, permission: &str) -> BoxedFunction {
    struct Permissioned {
        name: String,
        permission: String,
    }

    impl Function for Permissioned {
        fn manifest(&self) -> FunctionManifest {
            FunctionManifest {
                name: RString::from(self.name.as_str()),
                version: RString::from("0.0.0"),
                description: RString::from("permissioned test function"),
                // Deliberately the *closed* legacy value: if the host were
                // still reading `visibility`, every call below would 404 and
                // these assertions would fail loudly rather than pass by luck.
                visibility: Visibility::Private,
                role: RString::new(),
                method: HttpMethod::Post,
                permission: RString::from(self.permission.as_str()),
                admin: RString::new(),
                config_schema: RString::new(),
                input_schema: RString::new(),
                output_schema: RString::new(),
            }
        }

        fn invoke(
            &self,
            _host: HostApi_TO<'_, RBox<()>>,
            _input: RStr<'_>,
        ) -> RResult<RString, RString> {
            RResult::ROk(RString::from(json!({ "ran": true }).to_string()))
        }
    }

    Function_TO::from_value(
        Permissioned {
            name: name.to_string(),
            permission: permission.to_string(),
        },
        TD_Opaque,
    )
}

/// `permission` on a function manifest governs its endpoint, using the same
/// grammar a resource's `[permissions]` uses — including `member`, which
/// `Visibility` cannot express.
#[ntex::test]
async fn function_permissions_gate_endpoints_by_membership_and_role() {
    let db = TempDatabase::create("fnperms").await;
    let root = temp_dir("fnperms");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "\n[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                db.url
            ),
        )],
    );

    let state = load_state_with(
        &root,
        vec![
            permissioned_function("open", "public"),
            permissioned_function("signed_in", "authenticated"),
            permissioned_function("colleagues", "member"),
            permissioned_function("bosses", "role:admin"),
            permissioned_function("nobody", "private"),
        ],
    )
    .await;
    let app = init_http_app!(state);

    let call = |uri: &str, token: Option<&str>, org: Option<&str>| {
        let mut request = test::TestRequest::post()
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .set_payload("{}".to_string());
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(org) = org {
            request = request.header("x-organization", org.to_string());
        }
        request.to_request()
    };

    // Anonymous: only the public one answers.
    assert_eq!(
        test::call_service(&app, call("/api/functions/open", None, None))
            .await
            .status()
            .as_u16(),
        200
    );
    assert_eq!(
        test::call_service(&app, call("/api/functions/signed_in", None, None))
            .await
            .status()
            .as_u16(),
        401
    );
    assert_eq!(
        test::call_service(&app, call("/api/functions/colleagues", None, None))
            .await
            .status()
            .as_u16(),
        401
    );
    // A private function is absent, not forbidden — probing cannot enumerate it.
    assert_eq!(
        test::call_service(&app, call("/api/functions/nobody", None, None))
            .await
            .status()
            .as_u16(),
        404
    );

    // Signed in, but in no organisation yet.
    let registered = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ana@example.com","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    let ana = registered["token"].as_str().unwrap().to_string();

    assert_eq!(
        test::call_service(&app, call("/api/functions/signed_in", Some(&ana), None))
            .await
            .status()
            .as_u16(),
        200
    );
    // `member` needs an organisation, and there is none to resolve.
    assert_eq!(
        test::call_service(&app, call("/api/functions/colleagues", Some(&ana), None))
            .await
            .status()
            .as_u16(),
        403
    );

    // Creating an organisation makes its creator an admin of it.
    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"name":"Alpha","slug":"alpha"}).to_string()),
                &ana,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();

    for name in ["colleagues", "bosses"] {
        assert_eq!(
            test::call_service(
                &app,
                call(&format!("/api/functions/{name}"), Some(&ana), Some(&org_id))
            )
            .await
            .status()
            .as_u16(),
            200,
            "an admin should be able to call {name}"
        );
    }

    // A plain member of the same organisation: `member` yes, `role:admin` no.
    let joined = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ben@example.com","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    let ben = joined["token"].as_str().unwrap().to_string();
    let ben_id = joined["user"]["id"].as_str().unwrap().to_string();

    test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .header("x-organization", org_id.as_str())
                .set_payload(json!({"user_id":ben_id,"role":"member"}).to_string()),
            &ana,
        )
        .to_request(),
    )
    .await;

    assert_eq!(
        test::call_service(
            &app,
            call("/api/functions/colleagues", Some(&ben), Some(&org_id))
        )
        .await
        .status()
        .as_u16(),
        200
    );
    assert_eq!(
        test::call_service(
            &app,
            call("/api/functions/bosses", Some(&ben), Some(&org_id))
        )
        .await
        .status()
        .as_u16(),
        403
    );

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
