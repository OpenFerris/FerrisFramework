use super::page::normalize_route;
use super::*;

#[test]
fn test_routes_are_normalised() {
    assert_eq!(normalize_route("about"), "/about");
    assert_eq!(normalize_route("/About/"), "/about");
    assert_eq!(normalize_route("/"), "/");
}

#[test]
fn test_lookup_is_case_insensitive() {
    let router = WebsiteRouter::new();
    router
        .register(WebPage::new("/About", "About", "hi").published())
        .unwrap();
    assert!(router.get("/about").is_some());
    assert!(router.get("/ABOUT").is_some());
}

#[test]
fn test_drafts_are_not_served() {
    let router = WebsiteRouter::new();
    router
        .register(WebPage::new("/draft", "Draft", "hidden"))
        .unwrap();
    assert!(router.get("/draft").is_none());
    assert!(router.get("/DRAFT").is_none());
}

#[test]
fn test_published_page_renders_with_meta() {
    let router = WebsiteRouter::new();
    router
        .register(
            WebPage::new("/about", "About <Us>", "<p>Hello</p>")
                .published()
                .with_meta("description", "Learn \"about\" us"),
        )
        .unwrap();

    let rendered = router.render("/about").unwrap();
    assert!(rendered.html.contains("<title>About &lt;Us&gt;</title>"));
    assert!(
        rendered
            .html
            .contains("<meta name=\"description\" content=\"Learn &quot;about&quot; us\">")
    );
    assert!(rendered.html.contains("<p>Hello</p>"));
}

#[test]
fn test_sitemap_only_includes_published() {
    let router = WebsiteRouter::new();
    router
        .register(WebPage::new("/", "Home", "hi").published())
        .unwrap();
    router
        .register(WebPage::new("/about", "About", "hi").published())
        .unwrap();
    router
        .register(WebPage::new("/draft", "Draft", "hi"))
        .unwrap();

    let sitemap = router.sitemap("https://example.com/");
    assert!(sitemap.contains("<loc>https://example.com/</loc>"));
    assert!(sitemap.contains("<loc>https://example.com/about</loc>"));
    assert!(!sitemap.contains("/draft"));
}

#[test]
fn test_published_page_from_a_stored_row_is_sanitised() {
    let row = ferris_core::value!({
        "route": "/About/",
        "title": "About",
        "main_section": "<p>Hi</p><script>alert(1)</script>",
        "content_type": "HTML",
        "published": true,
    });
    let page = WebPage::from_value(&row).expect("published page resolves");
    assert_eq!(page.route, "/about");
    let rendered = WebsiteRouter::new().render_page(&page);
    assert!(rendered.html.contains("<p>Hi</p>"));
    assert!(!rendered.html.contains("<script>"));

    let draft = ferris_core::value!({
        "route": "/about",
        "title": "About",
        "main_section": "<p>Hi</p>",
        "published": false,
    });
    assert!(WebPage::from_value(&draft).is_none());
}

#[test]
fn test_web_page_from_markdown() {
    let page = WebPage::from_markdown(
        "/about",
        "---\ntitle: About\ntemplate: layout/wide\ndescription: About us\n---\n# Hello\n",
    );
    assert_eq!(page.title, "About");
    assert_eq!(page.template, "layout/wide");
    assert_eq!(
        page.meta_tags.get("description").map(String::as_str),
        Some("About us")
    );
    assert!(page.content.contains("<h1>Hello</h1>"));
}

#[test]
fn test_blog_publish_and_tags() {
    let blog = Blog::new();
    blog.register(
        BlogPost::new("hello-world", "Hello", "Hi")
            .with_tag("news")
            .published(),
    )
    .unwrap();
    blog.register(BlogPost::new("draft", "Draft", "x")).unwrap();

    assert!(blog.get("hello-world").is_some());
    assert!(blog.get("draft").is_none());
    assert_eq!(blog.list_published().len(), 1);
    assert_eq!(blog.by_tag("NEWS").len(), 1);
    assert!(blog.register(BlogPost::new("bad slug!", "x", "y")).is_err());
}

#[test]
fn test_web_form_validation() {
    let form = WebForm::new(
        "Contact",
        "Lead",
        vec![
            WebFormField::new("email", "Email", "Email").required(),
            WebFormField::new("message", "Message", "Data").required(),
            WebFormField::new("age", "Age", "Int"),
        ],
    );

    // Missing required field.
    assert!(
        form.validate(&ferris_core::value!({ "email": "a@example.com" }))
            .is_err()
    );
    // Invalid email.
    assert!(
        form.validate(&ferris_core::value!({ "email": "nope", "message": "hi" }))
            .is_err()
    );
    // Unknown field.
    assert!(
        form.validate(&ferris_core::value!({ "email": "a@example.com", "message": "hi", "x": 1 }))
            .is_err()
    );

    let cleaned = form
        .validate(&ferris_core::value!({ "email": "a@example.com", "message": "hi" }))
        .unwrap();
    assert_eq!(
        cleaned.get("email").and_then(|v| v.as_str()),
        Some("a@example.com")
    );
}

#[test]
fn test_discussion_threading() {
    let discussion = Discussion::new();
    let root = discussion
        .add("/about", "user@example.com", "Hello", None)
        .unwrap();
    let reply = discussion
        .add("/about", "other@example.com", "Hi", Some(root.id.clone()))
        .unwrap();

    assert_eq!(discussion.list("/about").len(), 2);
    assert_eq!(discussion.replies(&root.id).len(), 1);
    assert_eq!(discussion.replies(&root.id)[0].id, reply.id);

    // Replies must share the parent's page, and the parent must exist.
    assert!(
        discussion
            .add("/other", "x@example.com", "y", Some(root.id.clone()))
            .is_err()
    );
    assert!(
        discussion
            .add("/about", "x@example.com", "y", Some("missing".to_string()))
            .is_err()
    );
    // Empty bodies are rejected.
    assert!(
        discussion
            .add("/about", "x@example.com", "   ", None)
            .is_err()
    );

    discussion.delete(&root.id).unwrap();
    assert_eq!(discussion.list("/about").len(), 1);
}
