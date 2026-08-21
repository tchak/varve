//! Subject: locale resolution from request headers — an
//! `Accept-Language` header reaching the page as French copy and a
//! French `lang` attribute. Uses the home page as its canvas, but the
//! subject is i18n, not the shell.

use crate::harness::{body_text, get, test_app};

#[tokio::test]
async fn home_renders_french_from_accept_language() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let response = router
        .handle(get("/", &[("accept-language", "fr-CH, en;q=0.7")]))
        .await;
    let html = body_text(response).await;
    assert!(
        html.contains("Veuillez vous connecter pour continuer."),
        "{html}"
    );
    assert!(html.contains("Se connecter"), "{html}");
    assert!(html.contains(r#"lang="fr""#), "{html}");
}
