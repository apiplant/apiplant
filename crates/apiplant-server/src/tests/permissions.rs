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

/// An organisation admin may bypass `owner` list/read scopes within their org.
#[ntex::test]
async fn org_admins_can_list_and_read_owner_scoped_rows_for_everyone() {
    let db = TempDatabase::create("org-admin-owner-bypass").await;
    let root = temp_dir("org-admin-owner-bypass");
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
                "resources/note.toml",
                r#"
[resource]
name = "note"

[permissions]
list   = "owner"
read   = "owner"
create = "member"
update = "owner"
delete = "owner"

[fields.title]
type = "string"
required = true

[fields.owner_id]
type = "reference"
references = "user"
required = true
"#,
            ),
        ],
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
    let member = read_json(test::call_service(&app, register("member@example.com")).await).await;
    let member_token = member["token"].as_str().unwrap().to_string();

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

    let as_admin = |req: test::TestRequest| {
        bearer(req.header("x-organization", org_id.clone()), &admin_token).to_request()
    };
    let as_member = |req: test::TestRequest| {
        bearer(req.header("x-organization", org_id.clone()), &member_token).to_request()
    };

    let joined = test::call_service(
        &app,
        as_admin(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"email":"member@example.com","role":"member"}).to_string()),
        ),
    )
    .await;
    assert_eq!(joined.status().as_u16(), 201);

    let mine = test::call_service(
        &app,
        as_admin(
            test::TestRequest::post()
                .uri("/api/note")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"Admin note"}).to_string()),
        ),
    )
    .await;
    assert_eq!(mine.status().as_u16(), 201);
    let mine_id = read_json(mine).await["id"].as_str().unwrap().to_string();

    let theirs = test::call_service(
        &app,
        as_member(
            test::TestRequest::post()
                .uri("/api/note")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"Member note"}).to_string()),
        ),
    )
    .await;
    assert_eq!(theirs.status().as_u16(), 201);
    let theirs_id = read_json(theirs).await["id"].as_str().unwrap().to_string();

    let listed = read_json(
        test::call_service(&app, as_admin(test::TestRequest::get().uri("/api/note"))).await,
    )
    .await;
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row["id"] == mine_id));
    assert!(rows.iter().any(|row| row["id"] == theirs_id));

    let fetched = read_json(
        test::call_service(
            &app,
            as_admin(test::TestRequest::get().uri(&format!("/api/note/{theirs_id}"))),
        )
        .await,
    )
    .await;
    assert_eq!(fetched["id"], theirs_id);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// Global owner-scoped resources stay narrowed to the active organisation.
#[ntex::test]
async fn org_admins_only_see_global_owner_rows_for_the_active_org() {
    let db = TempDatabase::create("global-owner-org-scope").await;
    let root = temp_dir("global-owner-org-scope");
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
                "resources/note.toml",
                r#"
[resource]
name = "note"
scope = "global"

[permissions]
list   = "owner"
read   = "owner"
create = "authenticated"
update = "owner"
delete = "owner"

[fields.title]
type = "string"
required = true

[fields.owner_id]
type = "reference"
references = "user"
required = true
"#,
            ),
        ],
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
    let member = read_json(test::call_service(&app, register("member@example.com")).await).await;
    let member_token = member["token"].as_str().unwrap().to_string();
    let outsider =
        read_json(test::call_service(&app, register("outsider@example.com")).await).await;
    let outsider_token = outsider["token"].as_str().unwrap().to_string();

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
    let org_a = read_json(test::call_service(&app, create_org(&admin_token, "Alpha")).await).await;
    let org_a_id = org_a["id"].as_str().unwrap().to_string();
    let org_b =
        read_json(test::call_service(&app, create_org(&outsider_token, "Beta")).await).await;
    let org_b_id = org_b["id"].as_str().unwrap().to_string();

    let as_admin = |req: test::TestRequest| {
        bearer(req.header("x-organization", org_a_id.clone()), &admin_token).to_request()
    };
    let as_outsider = |req: test::TestRequest| {
        bearer(
            req.header("x-organization", org_b_id.clone()),
            &outsider_token,
        )
        .to_request()
    };

    let joined = test::call_service(
        &app,
        as_admin(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"email":"member@example.com","role":"member"}).to_string()),
        ),
    )
    .await;
    assert_eq!(joined.status().as_u16(), 201);

    let create_note = |token: &str| {
        bearer(
            test::TestRequest::post()
                .uri("/api/note")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"Note"}).to_string()),
            token,
        )
        .to_request()
    };
    let mine = read_json(test::call_service(&app, create_note(&admin_token)).await).await;
    let member_note = read_json(test::call_service(&app, create_note(&member_token)).await).await;
    let outsider_note =
        read_json(test::call_service(&app, create_note(&outsider_token)).await).await;

    let listed = read_json(
        test::call_service(&app, as_admin(test::TestRequest::get().uri("/api/note"))).await,
    )
    .await;
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row["id"] == mine["id"]));
    assert!(rows.iter().any(|row| row["id"] == member_note["id"]));
    assert!(!rows.iter().any(|row| row["id"] == outsider_note["id"]));

    let refused = test::call_service(
        &app,
        as_admin(test::TestRequest::get().uri(&format!(
            "/api/note/{}",
            outsider_note["id"].as_str().unwrap()
        ))),
    )
    .await;
    assert_eq!(refused.status().as_u16(), 404);

    let outsider_reads_own = test::call_service(
        &app,
        as_outsider(test::TestRequest::get().uri(&format!(
            "/api/note/{}",
            outsider_note["id"].as_str().unwrap()
        ))),
    )
    .await;
    assert_eq!(outsider_reads_own.status().as_u16(), 200);

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
                "resources/post.toml",
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
                "resources/user.toml",
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

/// Roles are a set, `admin` satisfies all of them, and nobody can demote
/// themselves out of administering an organisation.
#[ntex::test]
async fn roles_are_a_set_that_admins_hold_all_of_and_cannot_resign_from() {
    let db = TempDatabase::create("roles").await;
    let root = temp_dir("roles");
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
                // A resource only a `buyer` may create: the role check under test.
                "resources/order.toml",
                r#"
[resource]
name = "order"
scope = "organization"

[permissions]
list   = "member"
read   = "member"
create = "role:buyer"
update = "role:buyer"
delete = "role:admin"

[fields.reference]
type = "string"
"#,
            ),
        ],
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
    let founder = read_json(test::call_service(&app, register("founder@example.com")).await).await;
    let founder_token = founder["token"].as_str().unwrap().to_string();
    let staff = read_json(test::call_service(&app, register("staff@example.com")).await).await;
    let staff_token = staff["token"].as_str().unwrap().to_string();

    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({ "name": "Acme" }).to_string()),
                &founder_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();

    let as_founder = |req: test::TestRequest| {
        bearer(req.header("x-organization", org_id.clone()), &founder_token).to_request()
    };
    let as_staff = |req: test::TestRequest| {
        bearer(req.header("x-organization", org_id.clone()), &staff_token).to_request()
    };

    // The founder is the admin, and holds `buyer` without anyone granting it.
    let created = test::call_service(
        &app,
        as_founder(
            test::TestRequest::post()
                .uri("/api/order")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "reference": "PO-1" }).to_string()),
        ),
    )
    .await;
    assert_eq!(created.status().as_u16(), 201);

    // Add the staff member with a primary role that is not `buyer`.
    let membership = read_json(
        test::call_service(
            &app,
            as_founder(
                test::TestRequest::post()
                    .uri("/api/membership")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(
                        json!({ "email": "staff@example.com", "role": "support" }).to_string(),
                    ),
            ),
        )
        .await,
    )
    .await;
    let staff_membership = membership["id"].as_str().unwrap().to_string();

    // `support` is not `buyer`, and a plain member does not inherit anything.
    let refused = test::call_service(
        &app,
        as_staff(
            test::TestRequest::post()
                .uri("/api/order")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "reference": "PO-2" }).to_string()),
        ),
    )
    .await;
    assert_eq!(refused.status().as_u16(), 403);

    // A second role, alongside the first rather than instead of it.
    let granted = test::call_service(
        &app,
        as_founder(
            test::TestRequest::post()
                .uri("/api/membership_role")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({ "membership_id": staff_membership, "role": "buyer" }).to_string(),
                ),
        ),
    )
    .await;
    assert_eq!(granted.status().as_u16(), 201);
    let grant_id = read_json(granted).await["id"].as_str().unwrap().to_string();

    let allowed = test::call_service(
        &app,
        as_staff(
            test::TestRequest::post()
                .uri("/api/order")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "reference": "PO-3" }).to_string()),
        ),
    )
    .await;
    assert_eq!(allowed.status().as_u16(), 201);

    // …and the role they already had is still theirs.
    let held = read_json(
        test::call_service(
            &app,
            as_founder(test::TestRequest::get().uri("/api/membership_role")),
        )
        .await,
    )
    .await;
    assert_eq!(held.as_array().unwrap().len(), 1);
    assert_eq!(
        read_json(
            test::call_service(
                &app,
                as_founder(
                    test::TestRequest::get().uri(&format!("/api/membership/{staff_membership}"))
                ),
            )
            .await
        )
        .await["role"],
        "support"
    );

    // Granting the same role twice is a trap, not a grant: revoking the visible
    // copy would appear to do nothing.
    let again = test::call_service(
        &app,
        as_founder(
            test::TestRequest::post()
                .uri("/api/membership_role")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({ "membership_id": staff_membership, "role": "buyer" }).to_string(),
                ),
        ),
    )
    .await;
    assert_eq!(again.status().as_u16(), 409);

    // Nor may the primary role be shadowed by a duplicate grant.
    let shadow = test::call_service(
        &app,
        as_founder(
            test::TestRequest::post()
                .uri("/api/membership_role")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({ "membership_id": staff_membership, "role": "support" }).to_string(),
                ),
        ),
    )
    .await;
    assert_eq!(shadow.status().as_u16(), 409);

    // The admin may take a role off somebody else.
    let revoked = test::call_service(
        &app,
        as_founder(test::TestRequest::delete().uri(&format!("/api/membership_role/{grant_id}"))),
    )
    .await;
    assert_eq!(revoked.status().as_u16(), 204);
    let refused_again = test::call_service(
        &app,
        as_staff(
            test::TestRequest::post()
                .uri("/api/order")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "reference": "PO-4" }).to_string()),
        ),
    )
    .await;
    assert_eq!(refused_again.status().as_u16(), 403);

    // But not off themselves — by demotion…
    let own_membership = read_json(
        test::call_service(
            &app,
            as_founder(test::TestRequest::get().uri("/api/membership")),
        )
        .await,
    )
    .await;
    let founder_membership = own_membership
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["role"] == "admin")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let self_demote = test::call_service(
        &app,
        as_founder(
            test::TestRequest::patch()
                .uri(&format!("/api/membership/{founder_membership}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "role": "support" }).to_string()),
        ),
    )
    .await;
    assert_eq!(self_demote.status().as_u16(), 403);

    // …or by walking out, which would leave the organisation unadministrable.
    let self_remove = test::call_service(
        &app,
        as_founder(
            test::TestRequest::delete().uri(&format!("/api/membership/{founder_membership}")),
        ),
    )
    .await;
    assert_eq!(self_remove.status().as_u16(), 403);

    // Editing their own membership is fine as long as `admin` survives it.
    let harmless = test::call_service(
        &app,
        as_founder(
            test::TestRequest::patch()
                .uri(&format!("/api/membership/{founder_membership}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "role": "admin" }).to_string()),
        ),
    )
    .await;
    assert_eq!(harmless.status().as_u16(), 200);

    // A second admin may remove the first: only *self*-removal is refused, so
    // an organisation always keeps at least one administrator.
    let promote = test::call_service(
        &app,
        as_founder(
            test::TestRequest::patch()
                .uri(&format!("/api/membership/{staff_membership}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "role": "admin" }).to_string()),
        ),
    )
    .await;
    assert_eq!(promote.status().as_u16(), 200);

    let demote_other = test::call_service(
        &app,
        as_staff(
            test::TestRequest::patch()
                .uri(&format!("/api/membership/{founder_membership}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({ "role": "support" }).to_string()),
        ),
    )
    .await;
    assert_eq!(demote_other.status().as_u16(), 200);

    db.cleanup().await;
}

fn contains_org(listed: &Value, id: &str) -> bool {
    listed
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"].as_str() == Some(id))
}

/// The whole `org_class` contract, end to end.
///
/// A class decides which `@org_class=` permissions apply inside an
/// organisation, so who may write it is a deployment setting rather than a
/// row-level one — and whoever it names has to be able to reach organisations
/// they are not in, or the only classes they could set would be their own.
#[ntex::test]
async fn an_org_class_editor_sees_every_organisation_and_may_class_only_that() {
    let db = TempDatabase::create("orgclass").await;
    let root = temp_dir("orgclass");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n\n\
                 [organization]\norg_class_editors = \"member@org_class=admin\"\n",
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    // The back office has to start somewhere: nothing over HTTP can write the
    // first class, which is the point of the column being server-owned.
    let app = init_http_app!(state);

    let register = |email: &'static str| {
        req_json(
            "POST",
            "/api/auth/register",
            json!({ "email": email, "password": "pw" }),
        )
    };
    let staff = read_json(test::call_service(&app, register("staff@example.com")).await).await;
    let staff_token = staff["token"].as_str().unwrap().to_string();
    let outsider =
        read_json(test::call_service(&app, register("outsider@example.com")).await).await;
    let outsider_token = outsider["token"].as_str().unwrap().to_string();

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
    let ops = read_json(test::call_service(&app, create_org(&staff_token, "Ops")).await).await;
    let ops_id = ops["id"].as_str().unwrap().to_string();
    let theirs =
        read_json(test::call_service(&app, create_org(&outsider_token, "Theirs")).await).await;
    let theirs_id = theirs["id"].as_str().unwrap().to_string();

    // Creating an organisation does not class it, however the body was
    // written: the column is stripped like the tenant column is.
    assert_eq!(ops["org_class"], Value::Null);

    let patch_class = |token: &str, org: &str, active: &str, body: Value| {
        bearer(
            test::TestRequest::patch()
                .uri(&format!("/api/organization/{org}"))
                .header("x-organization", active.to_string())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(body.to_string()),
            token,
        )
        .to_request()
    };

    // Nobody is in an `admin`-class organisation yet, so nobody may class one:
    // the request succeeds as an ordinary update and the column is untouched.
    let attempt = read_json(
        test::call_service(
            &app,
            patch_class(
                &staff_token,
                &ops_id,
                &ops_id,
                json!({ "name": "Ops", "org_class": "admin" }),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(attempt["org_class"], Value::Null);

    // The operator classes the back office directly, which is what seed data
    // or SQL is for.
    state
        .db
        .raw_json(
            &format!(
                "UPDATE {} SET org_class = 'admin' WHERE id = $1::uuid",
                state.table("organization").unwrap()
            ),
            &[Value::String(ops_id.clone())],
        )
        .await
        .unwrap();

    // Now the list widens: a class editor sees every organisation, including
    // one they have no membership of, because they cannot class what they
    // cannot find. The header is what makes them one — the setting is answered
    // against the organisation selected, here and everywhere else.
    let listed = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get()
                    .uri("/api/organization")
                    .header("x-organization", ops_id.clone()),
                &staff_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert!(contains_org(&listed, &theirs_id));

    // Acting from anywhere else, the same account sees only its own again.
    let narrowed = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get().uri("/api/organization"),
                &staff_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert!(!contains_org(&narrowed, &theirs_id));

    // …and only for them: everybody else still sees their own.
    let theirs_view = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get().uri("/api/organization"),
                &outsider_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert!(!contains_org(&theirs_view, &ops_id));

    // The class editor may class an organisation they are not a member of.
    let classed = test::call_service(
        &app,
        patch_class(
            &staff_token,
            &theirs_id,
            &ops_id,
            json!({ "org_class": "customer" }),
        ),
    )
    .await;
    assert_eq!(classed.status().as_u16(), 200);
    assert_eq!(read_json(classed).await["org_class"], "customer");

    // But nothing else about it: a rename is not part of the bargain, and
    // smuggling one alongside the class does not make it one either.
    for body in [
        json!({ "name": "Hijacked" }),
        json!({ "name": "Hijacked", "org_class": "customer" }),
    ] {
        let refused =
            test::call_service(&app, patch_class(&staff_token, &theirs_id, &ops_id, body)).await;
        assert_eq!(refused.status().as_u16(), 404);
    }

    // And the setting is answered against the organisation *selected*: the
    // same person, acting somewhere unclassed, may not class anything.
    let elsewhere =
        read_json(test::call_service(&app, create_org(&staff_token, "Side project")).await).await;
    let elsewhere_id = elsewhere["id"].as_str().unwrap().to_string();
    let refused = test::call_service(
        &app,
        patch_class(
            &staff_token,
            &theirs_id,
            &elsewhere_id,
            json!({ "org_class": "pwned" }),
        ),
    )
    .await;
    assert_eq!(refused.status().as_u16(), 404);
}

/// `[organization] default_org_class` covers both doors an organisation comes
/// through, so "every new organisation" is not quietly "every one with a
/// `POST /organization` behind it".
#[ntex::test]
async fn a_default_class_is_stamped_on_every_new_organisation() {
    let db = TempDatabase::create("orgclassdefault").await;
    let root = temp_dir("orgclassdefault");
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n\n\
                 [organization]\ndefault_org_class = \"customer\"\n",
                db.url
            ),
        )],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    // The personal organisation an account is created with.
    let registered = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({ "email": "sam@example.com", "password": "pw" }),
            ),
        )
        .await,
    )
    .await;
    let token = registered["token"].as_str().unwrap().to_string();
    let personal = read_json(
        test::call_service(
            &app,
            bearer(test::TestRequest::get().uri("/api/organization"), &token).to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(personal[0]["org_class"], "customer");

    // …and one made over the API, where a class in the body is still refused:
    // the default fills the column, it does not open it.
    let created = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({ "name": "Acme", "org_class": "admin" }).to_string()),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(created["org_class"], "customer");
}

/// One action, three answers — the whole point of a rule set.
///
/// A family's chores: a parent edits everyone's, a kid edits only their own,
/// and a grounded member edits none. None of that is expressible as a single
/// level, and all of it has to hold at once.
#[ntex::test]
async fn one_action_can_answer_differently_for_each_role() {
    let db = TempDatabase::create("ruleset").await;
    let root = temp_dir("ruleset");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    "[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n\n\
                     [organization]\ndefault_org_class = \"family\"\n",
                    db.url
                ),
            ),
            (
                "resources/chore.toml",
                r#"
[resource]
name = "chore"
scope = "organization"

[permissions]
list   = "member"
read   = "member"
create = "member"
delete = "role:parent"

[permissions.update]
allow = "role:parent@org_class=family"
own   = "role:kid@org_class=family"
deny  = "role:grounded"

[fields.title]
type = "string"

[fields.owner_id]
type = "uuid"
"#,
            ),
        ],
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
    let parent = read_json(test::call_service(&app, register("parent@example.com")).await).await;
    let parent_token = parent["token"].as_str().unwrap().to_string();
    let kid = read_json(test::call_service(&app, register("kid@example.com")).await).await;
    let kid_token = kid["token"].as_str().unwrap().to_string();
    let teen = read_json(test::call_service(&app, register("teen@example.com")).await).await;
    let teen_token = teen["token"].as_str().unwrap().to_string();

    let org = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/organization")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({ "name": "Family" }).to_string()),
                &parent_token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    // `default_org_class` classes it, which is what the `@org_class=family`
    // half of every clause is answered against.
    assert_eq!(org["org_class"], "family");

    let as_parent = |req: test::TestRequest| {
        bearer(req.header("x-organization", org_id.clone()), &parent_token).to_request()
    };
    let as_kid = |req: test::TestRequest| {
        bearer(req.header("x-organization", org_id.clone()), &kid_token).to_request()
    };
    let as_teen = |req: test::TestRequest| {
        bearer(req.header("x-organization", org_id.clone()), &teen_token).to_request()
    };

    for (email, role) in [("kid@example.com", "kid"), ("teen@example.com", "kid")] {
        let added = test::call_service(
            &app,
            as_parent(
                test::TestRequest::post()
                    .uri("/api/membership")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({ "email": email, "role": role }).to_string()),
            ),
        )
        .await;
        assert_eq!(added.status().as_u16(), 201);
    }

    let chore = |req: test::TestRequest, title: &str| {
        req.uri("/api/chore")
            .header(CONTENT_TYPE, "application/json")
            .set_payload(json!({ "title": title }).to_string())
    };
    let kids_chore = read_json(
        test::call_service(&app, as_kid(chore(test::TestRequest::post(), "dishes"))).await,
    )
    .await;
    let kids_chore_id = kids_chore["id"].as_str().unwrap().to_string();
    let teens_chore = read_json(
        test::call_service(&app, as_teen(chore(test::TestRequest::post(), "lawn"))).await,
    )
    .await;
    let teens_chore_id = teens_chore["id"].as_str().unwrap().to_string();

    let rename = |id: &str, to: &str| {
        test::TestRequest::patch()
            .uri(&format!("/api/chore/{id}"))
            .header(CONTENT_TYPE, "application/json")
            .set_payload(json!({ "title": to }).to_string())
    };

    // The `own` clause: a kid's own row, yes.
    let mine = test::call_service(&app, as_kid(rename(&kids_chore_id, "washing up"))).await;
    assert_eq!(mine.status().as_u16(), 200);

    // …and somebody else's, no. Filtered out rather than refused, which is what
    // ownership scoping has always done: it is indistinguishable from missing.
    let theirs = test::call_service(&app, as_kid(rename(&teens_chore_id, "mow"))).await;
    assert_eq!(theirs.status().as_u16(), 404);

    // The `allow` clause is the wider answer to the same question.
    let parental = test::call_service(&app, as_parent(rename(&teens_chore_id, "mow the lawn"))).await;
    assert_eq!(parental.status().as_u16(), 200);

    // A second role that is denied outranks the one that allows: the teen still
    // holds `kid`, and would otherwise still be editing their own row.
    let membership = read_json(
        test::call_service(
            &app,
            as_parent(test::TestRequest::get().uri("/api/membership?role=kid")),
        )
        .await,
    )
    .await;
    let teen_membership = membership
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["email"] == "teen@example.com" || row["user_id"] == teen["user"]["id"])
        .expect("the teen's membership")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let grounded = test::call_service(
        &app,
        as_parent(
            test::TestRequest::post()
                .uri("/api/membership_role")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({ "membership_id": teen_membership, "role": "grounded" }).to_string(),
                ),
        ),
    )
    .await;
    assert_eq!(grounded.status().as_u16(), 201);

    let refused = test::call_service(&app, as_teen(rename(&teens_chore_id, "nothing"))).await;
    assert_eq!(refused.status().as_u16(), 403);

    // The parent administers the organisation, so they hold `grounded` the way
    // an admin holds every role — and a denial must not read it that way, or
    // adding the role would have locked the family's own admin out.
    let still_allowed =
        test::call_service(&app, as_parent(rename(&teens_chore_id, "mow the lawn again"))).await;
    assert_eq!(still_allowed.status().as_u16(), 200);

    // Nothing in the set mentions an action's other verbs, so they keep the
    // levels they were given.
    let listed = read_json(
        test::call_service(&app, as_kid(test::TestRequest::get().uri("/api/chore"))).await,
    )
    .await;
    assert_eq!(listed.as_array().unwrap().len(), 2);
}
