use tera::Context;

use super::tera_with_overrides;

#[test]
fn override_replaces_only_the_template_of_the_same_name() {
    let tera =
        tera_with_overrides(&[("navbar.html.j2".to_string(), "<nav>mine</nav>".to_string())])
            .expect("valid override");

    let navbar = tera.render("navbar.html.j2", &Context::new()).unwrap();
    assert_eq!(navbar, "<nav>mine</nav>");
    assert!(tera.get_template_names().any(|n| n == "footer.html.j2"));
}

#[test]
fn invalid_override_is_an_error() {
    let result = tera_with_overrides(&[("navbar.html.j2".to_string(), "{% if %}".to_string())]);
    assert!(result.is_err());
}
