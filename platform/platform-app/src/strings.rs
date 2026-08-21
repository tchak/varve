//! The P0 UI strings: English and French, as in-code `(id, source)`
//! tables compiled into [`platform_i18n::Catalogs`] at startup.
//!
//! **PROVISIONAL.** The catalog *container* format — TOML, JSON,
//! directories of `.mf2` files — is an open design point
//! (`platform_i18n::catalog` module docs), which is why
//! `platform-i18n` loads plain pairs and nothing else. These tables
//! are the P0 stopgap; once the format settles, the catalogs
//! themselves move to `platform-i18n` (PLATFORM.md P.3: "the MF2
//! catalogs (English + French)" belong there) and this module keeps
//! only the loading call.
//!
//! Sources are MessageFormat 2. French follows French typographic
//! convention: no-break space (U+00A0, written `\u{a0}` to keep it
//! visible) before `?` and `:`, and sober administrative register
//! with vouvoiement.

use platform_i18n::{Catalog, Catalogs};

use crate::i18n::DEFAULT_LOCALE;

/// English messages — also the fallback for any id a translation
/// misses, per the `[en]` fallback chain in [`catalogs`].
pub const EN: &[(&str, &str)] = &[
    ("app.title", "Varve"),
    ("nav.sign-in", "Sign in"),
    ("nav.sign-up", "Create an account"),
    ("nav.sign-out", "Sign out"),
    ("nav.signed-in-as", "Signed in as {$email}"),
    ("home.title", "Home"),
    ("home.greeting", "Hello, {$name}."),
    ("home.signed-out", "Please sign in to continue."),
    ("signin.title", "Sign in"),
    ("signin.submit", "Sign in"),
    (
        "signin.error.invalid-credentials",
        "Incorrect email address or password.",
    ),
    ("signin.signup-link", "No account yet? Create one."),
    ("signup.title", "Create an account"),
    ("signup.submit", "Create account"),
    (
        "signup.error.email-taken",
        "An account with this email address already exists.",
    ),
    ("signup.signin-link", "Already have an account? Sign in."),
    ("form.name", "Name"),
    ("form.email", "Email address"),
    ("form.password", "Password"),
    ("settings.title", "Settings"),
    ("settings.tab.account", "Account"),
    ("settings.tab.security", "Security"),
    ("settings.account.profile.title", "User profile"),
    ("settings.security.sessions.title", "Active sessions"),
    ("settings.security.sessions.current", "Current session"),
    ("settings.security.sessions.revoke", "Revoke session"),
    ("settings.security.sessions.unknown", "Unknown"),
    ("settings.security.sessions.created", "Signed in on {$date}"),
    ("settings.security.sessions.expires", "Expires on {$date}"),
    ("error.not-found", "Page not found."),
];

/// French messages.
pub const FR: &[(&str, &str)] = &[
    ("app.title", "Varve"),
    ("nav.sign-in", "Se connecter"),
    ("nav.sign-up", "Créer un compte"),
    ("nav.sign-out", "Se déconnecter"),
    ("nav.signed-in-as", "Connecté(e) en tant que {$email}"),
    ("home.title", "Accueil"),
    ("home.greeting", "Bonjour {$name}."),
    ("home.signed-out", "Veuillez vous connecter pour continuer."),
    ("signin.title", "Se connecter"),
    ("signin.submit", "Se connecter"),
    (
        "signin.error.invalid-credentials",
        "Adresse électronique ou mot de passe incorrect.",
    ),
    (
        "signin.signup-link",
        "Pas encore de compte\u{a0}? Créez-en un.",
    ),
    ("signup.title", "Créer un compte"),
    ("signup.submit", "Créer le compte"),
    (
        "signup.error.email-taken",
        "Un compte existe déjà avec cette adresse électronique.",
    ),
    (
        "signup.signin-link",
        "Vous avez déjà un compte\u{a0}? Connectez-vous.",
    ),
    ("form.name", "Nom"),
    ("form.email", "Adresse électronique"),
    ("form.password", "Mot de passe"),
    ("settings.title", "Paramètres"),
    ("settings.tab.account", "Compte"),
    ("settings.tab.security", "Sécurité"),
    ("settings.account.profile.title", "Profil de l'utilisateur"),
    ("settings.security.sessions.title", "Sessions actives"),
    ("settings.security.sessions.current", "Session actuelle"),
    ("settings.security.sessions.revoke", "Révoquer la session"),
    ("settings.security.sessions.unknown", "Inconnu"),
    ("settings.security.sessions.created", "Ouverte le {$date}"),
    ("settings.security.sessions.expires", "Expire le {$date}"),
    ("error.not-found", "Page introuvable."),
];

/// Compiles both catalogs with the `[en]` fallback chain: a message
/// missing from the French catalog renders in English (with English
/// CLDR data — the catalogs format in the locale that *holds* the
/// message). Called once at startup by [`crate::router`]; a table
/// that fails to compile is a build defect, so this panics with the
/// full per-id error list rather than limping on.
pub fn catalogs() -> Catalogs {
    let en = platform_i18n::locale(DEFAULT_LOCALE).expect("supported locale literals parse");
    let fr = platform_i18n::locale("fr").expect("supported locale literals parse");
    let mut catalogs = Catalogs::new(vec![en.clone()]);
    catalogs.insert(
        en,
        Catalog::from_pairs(EN.iter().copied()).expect("the English string table compiles"),
    );
    catalogs.insert(
        fr,
        Catalog::from_pairs(FR.iter().copied()).expect("the French string table compiles"),
    );
    catalogs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_string_tables_compile() {
        // The compile check the module promises: every source in both
        // tables is valid MF2 — `from_pairs` reports all failures
        // with their ids, so a red run names the broken messages.
        Catalog::from_pairs(EN.iter().copied()).expect("English catalog");
        Catalog::from_pairs(FR.iter().copied()).expect("French catalog");
    }

    #[test]
    fn tables_cover_the_same_ids() {
        // The [en] fallback chain makes a missing French id render in
        // English silently; catching drift here keeps that fallback
        // for emergencies, not routine.
        fn ids<'t>(table: &'t [(&'t str, &'t str)]) -> Vec<&'t str> {
            let mut ids: Vec<&str> = table.iter().map(|(id, _)| *id).collect();
            ids.sort_unstable();
            ids
        }
        assert_eq!(ids(EN), ids(FR));
    }

    #[test]
    fn no_duplicate_ids_within_a_table() {
        // `from_pairs` keeps the last occurrence like a map insert; a
        // duplicate would mask an earlier message without a trace.
        for table in [EN, FR] {
            let mut ids: Vec<&str> = table.iter().map(|(id, _)| *id).collect();
            let before = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(before, ids.len());
        }
    }
}
