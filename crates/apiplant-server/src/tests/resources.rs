//! Multitenancy, relationships, permissions, migrations and schema overrides
//! over the generic CRUD endpoints.

use super::*;

#[ntex::test]
async fn multitenancy_relationships_permissions_and_constraints_work_end_to_end() {
    let db = TempDatabase::create("multi").await;
    let root = temp_dir("multi");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &format!(
                    r#"
[server]
base_path = "/api"

[database]
url = "{}"
"#,
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
update = "owner"
delete = "role:admin"

[fields.title]
type = "string"
required = true
unique = true

[fields.owner_id]
type = "reference"
references = "user"
"#,
            ),
            (
                "models/comment.toml",
                r#"
[resource]
name = "comment"

[permissions]
list = "member"
read = "member"
create = "member"
update = "owner"
delete = "role:admin"

[fields.body]
type = "text"
required = true

[fields.post_id]
type = "reference"
references = "post"
required = true
on_delete = "cascade"

[fields.owner_id]
type = "reference"
references = "user"
"#,
            ),
            (
                "models/plan.toml",
                r#"
[resource]
name = "plan"
scope = "global"

[permissions]
list = "public"
read = "public"
create = "private"
update = "private"
delete = "private"

[fields.name]
type = "string"
"#,
            ),
        ],
    );

    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let alice_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"alice@example.com","password":"pw"}),
        ),
    )
    .await;
    let alice = read_json(alice_reg).await;
    let alice_token = alice["token"].as_str().unwrap().to_string();
    let alice_id = alice["user"]["id"].as_str().unwrap().to_string();

    let org_a_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/organization")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"Acme","slug":"acme"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_a_resp.status().as_u16(), 201);
    let org_a = read_json(org_a_resp).await;
    let org_a_id = org_a["id"].as_str().unwrap().to_string();

    let post_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(
                    json!({
                        "title":"hello",
                        "owner_id": Uuid::new_v4(),
                        "organization_id": Uuid::new_v4()
                    })
                    .to_string(),
                ),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(post_resp.status().as_u16(), 201);
    let post = read_json(post_resp).await;
    let post_id = post["id"].as_str().unwrap().to_string();
    assert_eq!(post["owner_id"], alice_id);
    assert_eq!(post["organization_id"], org_a_id);

    let second_post_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"later"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(second_post_resp.status().as_u16(), 201);
    let second_post = read_json(second_post_resp).await;
    let second_post_id = second_post["id"].as_str().unwrap().to_string();

    let duplicate_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/post")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"hello"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(duplicate_resp.status().as_u16(), 409);

    let comment_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/comment")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"body":"nice","post_id":post_id}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(comment_resp.status().as_u16(), 201);

    let bad_comment_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/comment")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"body":"oops","post_id":Uuid::new_v4()}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bad_comment_resp.status().as_u16(), 400);

    let paged_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri("/api/post?limit=1&offset=1"),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(paged_resp.status().as_u16(), 200);
    let paged = read_json(paged_resp).await;
    let page = paged.as_array().unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["id"], post_id);

    let update_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::put()
                .uri(&format!("/api/post/{post_id}"))
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"hello-updated"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(update_resp.status().as_u16(), 200);
    let updated = read_json(update_resp).await;
    assert_eq!(updated["title"], "hello-updated");

    let bob_reg = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"bob@example.com","password":"pw"}),
        ),
    )
    .await;
    let bob = read_json(bob_reg).await;
    let bob_token = bob["token"].as_str().unwrap().to_string();
    let bob_id = bob["user"]["id"].as_str().unwrap().to_string();

    let org_b_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/organization")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"name":"Beta","slug":"beta"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_b_resp.status().as_u16(), 201);
    let org_b = read_json(org_b_resp).await;
    let org_b_id = org_b["id"].as_str().unwrap().to_string();

    let isolated_resp = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/post"), &bob_token).to_request(),
    )
    .await;
    assert_eq!(isolated_resp.status().as_u16(), 200);
    assert_eq!(read_json(isolated_resp).await, json!([]));

    let plan_resp =
        test::call_service(&app, test::TestRequest::get().uri("/api/plan").to_request()).await;
    assert_eq!(plan_resp.status().as_u16(), 200);
    assert_eq!(read_json(plan_resp).await, json!([]));

    let membership_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":bob_id,"role":"member"}).to_string()),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(membership_resp.status().as_u16(), 201);
    let membership = read_json(membership_resp).await;
    assert_eq!(membership["organization_id"], org_a_id);

    let multi_org_resp = test::call_service(
        &app,
        bearer(test::TestRequest::get().uri("/api/post"), &bob_token).to_request(),
    )
    .await;
    assert_eq!(multi_org_resp.status().as_u16(), 403);
    assert_eq!(
        read_json(multi_org_resp).await["error"],
        "select an organisation with the X-Organization header"
    );

    let org_a_posts_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get()
                .uri("/api/post")
                .header("x-organization", org_a_id.as_str()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_a_posts_resp.status().as_u16(), 200);
    let org_a_posts = read_json(org_a_posts_resp).await;
    assert_eq!(org_a_posts.as_array().unwrap().len(), 2);

    let bob_update_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::patch()
                .uri(&format!("/api/post/{post_id}"))
                .header("x-organization", org_a_id.as_str())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"title":"nope"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_update_resp.status().as_u16(), 404);

    let bob_membership_write_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/membership")
                .header("x-organization", org_a_id.as_str())
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"user_id":alice_id,"role":"member"}).to_string()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(bob_membership_write_resp.status().as_u16(), 403);

    let expand_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get()
                .uri(&format!("/api/comment?expand=post,owner&post_id={post_id}")),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(expand_resp.status().as_u16(), 200);
    let expanded = read_json(expand_resp).await;
    let comment = &expanded.as_array().unwrap()[0];
    assert_eq!(comment["post"]["id"], post_id);
    assert_eq!(comment["owner"]["id"], alice_id);
    assert!(comment["owner"].get("password_hash").is_none());

    let nested_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get().uri(&format!("/api/post/{post_id}/comment")),
            &alice_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(nested_resp.status().as_u16(), 200);
    assert_eq!(read_json(nested_resp).await.as_array().unwrap().len(), 1);

    let org_b_posts_resp = test::call_service(
        &app,
        bearer(
            test::TestRequest::get()
                .uri("/api/post")
                .header("x-organization", org_b_id.as_str()),
            &bob_token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(org_b_posts_resp.status().as_u16(), 200);
    assert_eq!(read_json(org_b_posts_resp).await, json!([]));

    assert_ne!(second_post_id, post_id);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
