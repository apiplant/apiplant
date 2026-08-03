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

/// A public function that emits three chunks and then returns a summary — the
/// shape every streaming function has.
fn streaming_function(name: &str) -> BoxedFunction {
    struct Streaming {
        name: String,
    }

    impl Function for Streaming {
        fn manifest(&self) -> FunctionManifest {
            FunctionManifest {
                name: RString::from(self.name.as_str()),
                version: RString::from("0.0.0"),
                description: RString::from("streaming test function"),
                visibility: Visibility::Public,
                role: RString::new(),
                method: HttpMethod::Post,
                permission: RString::from("public"),
                admin: RString::new(),
                config_schema: RString::new(),
                input_schema: RString::new(),
                output_schema: RString::new(),
            }
        }

        fn invoke(
            &self,
            host: HostApi_TO<'_, RBox<()>>,
            _input: RStr<'_>,
        ) -> RResult<RString, RString> {
            let mut delivered = 0;
            for chunk in ["one ", "two ", "three"] {
                if host.emit(RStr::from_str(chunk)) {
                    delivered += 1;
                }
            }
            RResult::ROk(RString::from(json!({ "chunks": delivered }).to_string()))
        }
    }

    Function_TO::from_value(
        Streaming {
            name: name.to_string(),
        },
        TD_Opaque,
    )
}

/// The same function answers both endpoints, and the difference is entirely in
/// what the caller gets: a JSON document on one, a stream of frames on the
/// other.
#[ntex::test]
async fn a_function_streams_what_it_emits_and_ends_with_what_it_returned() {
    let db = TempDatabase::create("fnstream").await;
    let root = temp_dir("fnstream");
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

    let state = load_state_with(&root, vec![streaming_function("narrate")]).await;
    let app = init_http_app!(state);

    let post = |uri: &str| {
        test::TestRequest::post()
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .set_payload("{}".to_string())
            .to_request()
    };

    // Streamed: one `delta` frame per chunk, in order, then the return value.
    let response = test::call_service(&app, post("/api/functions/narrate/stream")).await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    let body = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
    assert_eq!(
        body,
        "event: delta\ndata: {\"text\":\"one \"}\n\n\
         event: delta\ndata: {\"text\":\"two \"}\n\n\
         event: delta\ndata: {\"text\":\"three\"}\n\n\
         event: done\ndata: {\"result\":{\"chunks\":3}}\n\n"
    );

    // Plain: the chunks go nowhere, the caller gets the return value, and the
    // function is not told to stop — because its caller is still waiting for
    // exactly that value. `emit` answers "keep going?", not "did that land?".
    let response = test::call_service(&app, post("/api/functions/narrate")).await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(read_json(response).await, json!({ "chunks": 3 }));

    // The streaming endpoint is the same function, so it is the same door: a
    // name nobody registered is absent on both.
    assert_eq!(
        test::call_service(&app, post("/api/functions/nope/stream"))
            .await
            .status()
            .as_u16(),
        404
    );

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
