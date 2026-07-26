//! Regression tests for authorization holes: bodies that try to write the
//! columns the server stamps, and reads that try to arrive by the side door.

use super::*;

/// A body may not write the columns the server stamps.
///
/// The dangerous one is `organization_id` on update: the `WHERE` clause proves
/// the caller may touch the row *where it is now*, so accepting a new tenant
/// from the body would let an admin of one organisation push a row — a
/// membership granting themselves `admin` — into an organisation they have no
/// part in. The password column is the same shape of mistake: writable through
/// generic CRUD, it would let anyone with `update` on `user` set a hash they
/// know.
#[ntex::test]
async fn updates_cannot_move_a_row_between_organisations_or_write_a_password() {
    let db = TempDatabase::create("stamped").await;
    let root = temp_dir("stamped");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let register = |email: &'static str| {
        req_json(
            "POST",
            "/api/auth/register",
            json!({ "email": email, "password": "pw" }),
        )
    };
    let mallory = read_json(test::call_service(&app, register("mallory@example.com")).await).await;
    let mallory_token = mallory["token"].as_str().unwrap().to_string();
    let mallory_id = mallory["user"]["id"].as_str().unwrap().to_string();
    let victim = read_json(test::call_service(&app, register("victim@example.com")).await).await;
    let victim_token = victim["token"].as_str().unwrap().to_string();

    let create_org = |token: &str, name: &str| {
        bearer(
            test::TestRequest::post()
                .uri("/api/organization")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "name": name }).to_string()),
            token,
        )
        .to_request()
    };
    let mine = read_json(test::call_service(&app, create_org(&mallory_token, "Mine")).await).await;
    let mine_id = mine["id"].as_str().unwrap().to_string();
    let theirs =
        read_json(test::call_service(&app, create_org(&victim_token, "Theirs")).await).await;
    let theirs_id = theirs["id"].as_str().unwrap().to_string();

    // Mallory is an admin of her own org, so she may edit its memberships.
    let memberships = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get()
                    .uri("/api/membership")
                    .header("x-organization", mine_id.clone()),
                &mallory_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let membership_id = memberships[0]["id"].as_str().unwrap().to_string();

    let moved = test::call_service(
        &app,
        bearer(
            test::TestRequest::patch()
                .uri(&format!("/api/membership/{membership_id}"))
                .header("x-organization", mine_id.clone())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "organization_id": theirs_id }).to_string()),
            &mallory_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(moved.status().as_u16(), 200);
    // The request succeeds, but the tenant column is untouched: the membership
    // is still in Mallory's own organisation.
    assert_eq!(read_json(moved).await["organization_id"], mine_id);

    // And she has gained nothing in the victim's organisation.
    let probe = test::call_service(
        &app,
        bearer(
            test::TestRequest::get()
                .uri("/api/membership")
                .header("x-organization", theirs_id.clone()),
            &mallory_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(probe.status().as_u16(), 403);

    // A password hash submitted through generic CRUD is dropped, so the old
    // password still logs in and the forged one does not.
    let patched = test::call_service(
        &app,
        bearer(
            test::TestRequest::patch()
                .uri(&format!("/api/user/{mallory_id}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({
                        "display_name": "Mallory",
                        "password_hash": "$argon2id$v=19$m=1,t=1,p=1$c2FsdA$forged"
                    })
                    .to_string(),
                ),
            &mallory_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(patched.status().as_u16(), 200);
    let login = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/login",
            json!({ "email": "mallory@example.com", "password": "pw" }),
        ),
    )
    .await;
    assert_eq!(login.status().as_u16(), 200);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// `user` reads as `member`: people who share an organisation can see each
/// other, which is what makes a membership list expandable into names and
/// emails. Someone with no organisation in common stays invisible.
#[ntex::test]
async fn org_members_can_read_each_other_but_strangers_cannot() {
    let db = TempDatabase::create("user-comembers").await;
    let root = temp_dir("user-comembers");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let register = |email: &str| {
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email": email, "password": "pw"}),
        )
    };
    let admin = read_json(test::call_service(&app, register("admin@example.com")).await).await;
    let admin_token = admin["token"].as_str().unwrap().to_string();
    let colleague =
        read_json(test::call_service(&app, register("colleague@example.com")).await).await;
    let colleague_token = colleague["token"].as_str().unwrap().to_string();
    let colleague_id = colleague["user"]["id"].as_str().unwrap().to_string();
    let stranger =
        read_json(test::call_service(&app, register("stranger@example.com")).await).await;
    let stranger_token = stranger["token"].as_str().unwrap().to_string();

    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"name":"Acme"}).to_string()),
                &admin_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();

    let added = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header("x-organization", org_id.clone())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id": colleague_id, "role": "member"}).to_string()),
            &admin_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(added.status().as_u16(), 201);

    // The team page: memberships with the user inlined, not just an id.
    let members = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get()
                    .uri("/api/membership?expand=user")
                    .header("x-organization", org_id.clone()),
                &admin_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let colleague_row = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["user_id"] == colleague_id)
        .expect("the colleague's membership");
    assert_eq!(colleague_row["user"]["email"], "colleague@example.com");

    // Directly, too — and in both directions.
    let read_as = |token: &str, id: &str| {
        bearer(
            test::TestRequest::get().uri(&format!("/api/user/{id}")),
            token,
        )
        .to_request()
    };
    let seen = test::call_service(&app, read_as(&colleague_token, &colleague_id)).await;
    assert_eq!(seen.status().as_u16(), 200);
    let seen = test::call_service(&app, read_as(&admin_token, &colleague_id)).await;
    assert_eq!(seen.status().as_u16(), 200);

    // A user with no organisation in common sees neither the row nor the list.
    let refused = test::call_service(&app, read_as(&stranger_token, &colleague_id)).await;
    assert_eq!(refused.status().as_u16(), 404);
    let listed = read_json(
        test::call_service(
            &app,
            bearer(test::TestRequest::get().uri("/api/user"), &stranger_token).to_request(),
        )
        .await,
    )
    .await;
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1, "only themselves");
    assert_eq!(listed[0]["email"], "stranger@example.com");

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// Adding a teammate by email goes through the `apiplant_organization_join`
/// built-in, because the admin doing it cannot look that email up themselves.
#[ntex::test]
async fn a_member_is_added_by_email_through_the_builtin_hook() {
    let db = TempDatabase::create("join-hook").await;
    let root = temp_dir("join-hook");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let register = |email: &str| {
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email": email, "password": "pw"}),
        )
    };
    let admin = read_json(test::call_service(&app, register("boss@example.com")).await).await;
    let admin_token = admin["token"].as_str().unwrap().to_string();
    let newcomer =
        read_json(test::call_service(&app, register("New.Hire@example.com")).await).await;
    let newcomer_id = newcomer["user"]["id"].as_str().unwrap().to_string();

    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"name":"Acme"}).to_string()),
                &admin_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();

    let add = |body: serde_json::Value| {
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header("x-organization", org_id.clone())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(body.to_string()),
            &admin_token,
        )
        .to_request()
    };

    // The email is resolved server-side — and case-insensitively, since it is
    // typed by a human, not copied from a listing.
    let created = test::call_service(
        &app,
        add(json!({"email":"new.hire@example.com","role":"member"})),
    )
    .await;
    assert_eq!(created.status().as_u16(), 201);
    let created = read_json(created).await;
    assert_eq!(created["user_id"], newcomer_id);
    assert_eq!(created["role"], "member");
    // `email` was an instruction to the hook, not a column.
    assert!(created.get("email").is_none());

    // Adding them twice is refused rather than duplicated.
    let again = test::call_service(&app, add(json!({"email":"new.hire@example.com"}))).await;
    assert_eq!(again.status().as_u16(), 409);

    // An address nobody registered with, and a body naming nobody at all.
    let unknown = test::call_service(&app, add(json!({"email":"ghost@example.com"}))).await;
    assert_eq!(unknown.status().as_u16(), 404);
    let empty = test::call_service(&app, add(json!({"role":"member"}))).await;
    assert_eq!(empty.status().as_u16(), 422);

    // `user_id` still works: the hook resolves an identity, it does not replace
    // the field.
    let by_id = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header("x-organization", org_id.clone())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id": newcomer_id, "role":"member"}).to_string()),
            &admin_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(
        by_id.status().as_u16(),
        409,
        "already a member, by either name"
    );

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// `?expand=` is a read of the *target* resource and is authorized as one.
///
/// With `user` narrowed to `read = "owner"`, expanding the author of someone
/// else's post must not hand back the row `GET /user/{id}` would refuse.
#[ntex::test]
async fn expanding_a_relation_respects_the_target_permissions() {
    let db = TempDatabase::create("expand-auth").await;
    let root = temp_dir("expand-auth");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n",
                    db.url
                ),
            ),
            (
                "models/post.toml",
                r#"
[resource]
name = "post"

[permissions]
list = "member"
read = "member"
create = "member"

[fields.title]
type = "string"
required = true

[fields.owner_id]
type = "reference"
references = "user"
"#,
            ),
            // `user` ships with `read = "member"`, which co-members pass. Narrow
            // it to `owner` so the expansion below has a policy to be refused by.
            (
                "models/user.toml",
                r#"
[resource]
name = "user"
scope = "global"
timestamps = true

[permissions]
list   = "authenticated"
read   = "owner"
create = "public"
update = "owner"
delete = "private"

[auth]
identity_field = "email"
password_field = "password_hash"

[fields.email]
type = "string"
required = true
unique = true

[fields.password_hash]
type = "string"
hidden = true
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let author = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"author@example.com","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    let author_token = author["token"].as_str().unwrap().to_string();
    let author_id = author["user"]["id"].as_str().unwrap().to_string();

    let reader = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"reader@example.com","password":"pw"}),
            ),
        )
        .await,
    )
    .await;
    let reader_token = reader["token"].as_str().unwrap().to_string();
    let reader_id = reader["user"]["id"].as_str().unwrap().to_string();

    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"name":"Newsroom"}).to_string()),
                &author_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();

    // The author is the org admin, so they add the reader as a member.
    let added = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header("x-organization", org_id.clone())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id": reader_id, "role": "member"}).to_string()),
            &author_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(added.status().as_u16(), 201);

    let created = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header("x-organization", org_id.clone())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"Scoop"}).to_string()),
            &author_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(created.status().as_u16(), 201);

    let expand = |token: &str| {
        bearer(
            test::TestRequest::get()
                .uri("/api/post?expand=owner")
                .header("x-organization", org_id.clone()),
            token,
        )
        .to_request()
    };

    // The author owns the row the relation points at, so they see it.
    let mine = read_json(test::call_service(&app, expand(&author_token)).await).await;
    assert_eq!(mine[0]["owner"]["id"], author_id);
    assert_eq!(mine[0]["owner"]["email"], "author@example.com");

    // The reader may list the post, but not read its author.
    let theirs = read_json(test::call_service(&app, expand(&reader_token)).await).await;
    assert_eq!(theirs[0]["title"], "Scoop");
    assert!(theirs[0]["owner"].is_null());

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
