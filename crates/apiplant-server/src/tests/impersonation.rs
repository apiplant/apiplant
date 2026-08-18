//! Acting as somebody else: the two doors, and the walls around each.

use super::*;

/// The app both cases run against: a `note` resource members may read, and a
/// back office whose admins may borrow anybody.
fn write_app(root: &Path, db_url: &str) {
    write_files(
        root,
        &[
            (
                "main.toml",
                &format!(
                    "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{db_url}\"\n\n\
                     [organization]\n\
                     global_admin_role = \"member@org_class=staff\"\n"
                ),
            ),
            (
                "resources/note.toml",
                "[resource]\nname = \"note\"\n\n[permissions]\nlist = \"member\"\n\
                 create = \"member\"\n\n[fields.title]\ntype = \"string\"\n",
            ),
        ],
    );
}

/// Register an account and bind `(user_id, token)`.
///
/// A macro rather than a function: `init_service` hands back an opaque service
/// type, and naming it in a signature costs more than the three lines it saves.
macro_rules! register {
    ($app:expr, $email:expr) => {{
        let created = read_json(
            test::call_service(
                &$app,
                req_json(
                    "POST",
                    "/api/auth/register",
                    json!({ "email": $email, "password": "pw" }),
                ),
            )
            .await,
        )
        .await;
        (
            created["user"]["id"].as_str().unwrap().to_string(),
            created["token"].as_str().unwrap().to_string(),
        )
    }};
}

/// The default door: an organisation's admin borrows one of its members, and
/// the session they get reaches that organisation and nothing else.
#[ntex::test]
async fn an_org_admin_may_act_as_a_member_of_their_own_organisation() {
    let db = TempDatabase::create("impersonate").await;
    let root = temp_dir("impersonate");
    write_app(&root, &db.url);

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let (_boss_id, boss_token) = register!(app, "boss@example.com");
    let (member_id, member_token) = register!(app, "member@example.com");

    // The boss's organisation, which they administer by having made it, and
    // the member's own private one, which the boss has nothing to do with.
    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({ "name": "Acme" }).to_string()),
                &boss_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let elsewhere = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({ "name": "Their side project" }).to_string()),
                &member_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let elsewhere_id = elsewhere["id"].as_str().unwrap().to_string();

    let impersonate = |token: &str, active: Option<&str>, body: Value| {
        let mut req = test::TestRequest::post()
            .uri("/api/auth/impersonate")
            .header(CONTENT_TYPE, "application/json")
            .set_payload(body.to_string());
        if let Some(active) = active {
            req = req.header("x-organization", active.to_string());
        }
        bearer(req, token).to_request()
    };

    // Not a member of the boss's organisation yet: not theirs to borrow.
    let refused = test::call_service(
        &app,
        impersonate(
            &boss_token,
            Some(&org_id),
            json!({ "user_id": member_id.clone() }),
        ),
    )
    .await;
    assert_eq!(refused.status().as_u16(), 403);

    state
        .db
        .raw_json(
            &format!(
                "INSERT INTO {} (id, organization_id, user_id, role) \
                 VALUES (gen_random_uuid(), $1::uuid, $2::uuid, 'member')",
                state.table("membership").unwrap()
            ),
            &[
                Value::String(org_id.clone()),
                Value::String(member_id.clone()),
            ],
        )
        .await
        .unwrap();

    let borrowed = read_json(
        test::call_service(
            &app,
            impersonate(
                &boss_token,
                Some(&org_id),
                json!({ "user_id": member_id.clone() }),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(borrowed["user_id"], member_id);
    assert_eq!(borrowed["organization_id"], org_id);
    let borrowed_token = borrowed["token"].as_str().unwrap().to_string();

    // The token acts as the member, and says who is really behind it.
    let me = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get().uri("/api/auth/me"),
                &borrowed_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(me["user_id"], member_id);
    assert!(me["impersonator"].as_str().is_some());
    assert_eq!(me["organization_id"], org_id);

    // The pin is the whole safety story: the member keeps a note in their
    // *other* organisation, and the borrowed session cannot be steered at it.
    let kept = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/note")
                .header("x-organization", elsewhere_id.clone())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "title": "Private to them" }).to_string()),
            &member_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(kept.status().as_u16(), 201);

    // Their own token finds it, with the header naming that organisation.
    let theirs = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get()
                    .uri("/api/note")
                    .header("x-organization", elsewhere_id.clone()),
                &member_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(theirs.as_array().unwrap().len(), 1);

    // The borrowed one, sending the very same header, does not: the header is
    // ignored rather than refused, and the answer is the pinned organisation's
    // notes — of which there are none.
    let steered = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get()
                    .uri("/api/note")
                    .header("x-organization", elsewhere_id.clone()),
                &borrowed_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert!(steered.as_array().unwrap().is_empty());

    // Nesting is refused: `act` always names a real person.
    let nested = test::call_service(
        &app,
        impersonate(
            &borrowed_token,
            Some(&org_id),
            json!({ "user_id": member_id.clone() }),
        ),
    )
    .await;
    assert_eq!(nested.status().as_u16(), 409);

    // And back again, with no second credential kept anywhere.
    let restored = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post().uri("/api/auth/impersonate/stop"),
                &borrowed_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(restored["impersonator"], Value::Null);
    let restored_token = restored["token"].as_str().unwrap().to_string();
    let me = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get().uri("/api/auth/me"),
                &restored_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(me["user_id"], restored["user_id"]);
    assert_eq!(me["impersonator"], Value::Null);

    // Somebody who is not the organisation's admin gets nowhere, member of it
    // or not.
    let refused = test::call_service(
        &app,
        impersonate(
            &member_token,
            Some(&org_id),
            json!({ "user_id": member_id.clone() }),
        ),
    )
    .await;
    assert!(matches!(refused.status().as_u16(), 400 | 403));
}

/// The wider door, and the fact that it is shut unless an app opens it.
#[ntex::test]
async fn only_a_global_admin_may_act_as_a_stranger() {
    let db = TempDatabase::create("impersonateany").await;
    let root = temp_dir("impersonateany");
    write_app(&root, &db.url);

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let (_staff_id, staff_token) = register!(app, "staff@example.com");
    let (stranger_id, stranger_token) = register!(app, "stranger@example.com");

    let ops = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({ "name": "Ops" }).to_string()),
                &staff_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let ops_id = ops["id"].as_str().unwrap().to_string();

    let borrow = |token: &str, target: &str| {
        bearer(
            test::TestRequest::post()
                .uri("/api/auth/impersonate")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "user_id": target }).to_string()),
            token,
        )
        .to_request()
    };

    // Before the class is stamped, nobody is staff and a stranger is a
    // stranger.
    let refused = test::call_service(&app, borrow(&staff_token, &stranger_id)).await;
    assert_eq!(refused.status().as_u16(), 403);

    state
        .db
        .raw_json(
            &format!(
                "UPDATE {} SET org_class = 'staff' WHERE id = $1::uuid",
                state.table("organization").unwrap()
            ),
            &[Value::String(ops_id.clone())],
        )
        .await
        .unwrap();

    let borrowed =
        read_json(test::call_service(&app, borrow(&staff_token, &stranger_id)).await).await;
    assert_eq!(borrowed["user_id"], stranger_id);
    // Not pinned: moving around the borrowed account's organisations is what
    // this door is for.
    assert_eq!(borrowed["organization_id"], Value::Null);

    // The stranger, meanwhile, is nobody's administrator and may borrow no
    // one — including the staff member who just borrowed them.
    let refused = test::call_service(&app, borrow(&stranger_token, &stranger_id)).await;
    assert!(matches!(refused.status().as_u16(), 400 | 403));

    // A borrowed session is never a back office, whoever borrowed it: the
    // deployment-wide powers do not travel into the account.
    let borrowed_token = borrowed["token"].as_str().unwrap().to_string();
    let me = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get().uri("/api/auth/me"),
                &borrowed_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(me["global_admin"], false);
}
