//! The P0 UI strings: English and French, as in-code `(id, source)`
//! tables compiled into [`platform_i18n::Catalogs`] at startup.
//!
//! **PROVISIONAL.** The catalog *container* format — TOML, JSON,
//! directories of `.mf2` files — is an open design point
//! (`platform_i18n::catalog` module docs), which is why
//! `platform-i18n` loads plain pairs and nothing else. These tables
//! are the P0 stopgap; once the format settles, the catalogs
//! themselves move to `platform-i18n` (design/platform.md P.3: "the MF2
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
    // The account menu's trigger is an icon-only button; this is its
    // accessible name (`aria-label`), not visible text.
    ("nav.account-menu", "Account menu"),
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
    ("form.language", "Language"),
    // The language options are endonyms — each language named in
    // itself, the i18n convention for language pickers — so both
    // tables carry them verbatim, one per supported locale id.
    ("locale.en", "English"),
    ("locale.fr", "Français"),
    ("settings.title", "Settings"),
    ("organizations.title", "Organizations"),
    ("organizations.list.title", "Your organizations"),
    (
        "organizations.list.empty",
        "You are not a member of any organization yet.",
    ),
    ("organizations.create.title", "Create an organization"),
    ("organizations.create.submit", "Create organization"),
    (
        "organizations.create.error.name-required",
        "Please enter a name.",
    ),
    (
        "organizations.create.error.slug-required",
        "Please enter an identifier.",
    ),
    (
        "organizations.create.error.slug-invalid",
        "The identifier may only contain lowercase letters, digits, and hyphens.",
    ),
    (
        "organizations.create.error.slug-taken",
        "This identifier is already taken.",
    ),
    ("form.slug", "Identifier"),
    ("form.slug.hint", "my-organization"),
    (
        "organization.created",
        "Created on {$date :date style=medium}",
    ),
    ("organization.members.title", "Members ({$count :integer})"),
    ("organization.teams.title", "Teams ({$count :integer})"),
    ("organization.teams.empty", "No teams yet."),
    ("organization.teams.manage", "Manage teams"),
    ("teams.title", "Teams"),
    ("teams.lead", "Teams of"),
    ("teams.list.title", "Teams"),
    ("teams.list.empty", "No teams yet."),
    ("teams.create.title", "Create a team"),
    ("teams.create.submit", "Create team"),
    ("teams.create.error.name-required", "Please enter a name."),
    ("organization.procedures.manage", "Manage procedures"),
    ("procedures.title", "Procedures"),
    ("procedures.lead", "Procedures of"),
    ("procedures.list.title", "Procedures"),
    ("procedures.list.empty", "No procedures yet."),
    ("procedures.create.title", "Create a procedure"),
    ("procedures.create.submit", "Create procedure"),
    (
        "procedures.create.error.title-required",
        "Please enter a title.",
    ),
    ("form.label", "Label"),
    ("procedure.lead", "A procedure of"),
    ("procedure.draft.title", "Next revision"),
    ("procedure.draft.badge", "Draft"),
    (
        "procedure.draft.none",
        "No draft yet. The schema editor starts one.",
    ),
    (
        "procedure.draft.summary",
        ".input {$columns :integer}\n\
         .input {$groups :integer}\n\
         .match $columns $groups\n\
         one one {{{$columns} column and {$groups} group, saved {$date :date style=long}.}}\n\
         one * {{{$columns} column and {$groups} groups, saved {$date :date style=long}.}}\n\
         * one {{{$columns} columns and {$groups} group, saved {$date :date style=long}.}}\n\
         * * {{{$columns} columns and {$groups} groups, saved {$date :date style=long}.}}",
    ),
    ("procedure.draft.edit", "Edit the schema"),
    ("schema.title", "Schema of {$procedure}"),
    ("schema.back", "Back to the procedure"),
    ("schema.tab.editor", "Editor"),
    ("schema.tab.preview", "Preview"),
    (
        "schema.preview.empty",
        "Nothing to preview yet. Add elements in the editor.",
    ),
    (
        "schema.preview.geometry",
        "Map input — not shown in the preview.",
    ),
    (
        "schema.state.none",
        "No draft yet: adding an element starts one.",
    ),
    (
        "schema.state.draft",
        ".input {$columns :integer}\n\
         .input {$groups :integer}\n\
         .match $columns $groups\n\
         one one {{{$columns} column, {$groups} group \u{2014} saved {$date :date style=long}.}}\n\
         one * {{{$columns} column, {$groups} groups \u{2014} saved {$date :date style=long}.}}\n\
         * one {{{$columns} columns, {$groups} group \u{2014} saved {$date :date style=long}.}}\n\
         * * {{{$columns} columns, {$groups} groups \u{2014} saved {$date :date style=long}.}}",
    ),
    ("schema.discard", "Discard the draft"),
    (
        "schema.discard.question",
        "Discard the whole draft? Every unpublished change is lost.",
    ),
    ("schema.discard.confirm", "Discard"),
    ("schema.discard.keep", "Keep editing"),
    ("schema.structure.title", "Structure"),
    (
        "schema.structure.empty",
        "The schema is empty. Add a column or a group to start.",
    ),
    ("schema.actions", "Actions"),
    ("schema.actions.for", "Actions for {$label}"),
    ("schema.actions.move-up", "Move up"),
    ("schema.actions.move-down", "Move down"),
    ("schema.actions.move-top", "Move to top"),
    ("schema.actions.move-bottom", "Move to bottom"),
    ("schema.actions.move-after", "Move after"),
    ("schema.actions.move-to", "Move to"),
    ("schema.actions.top-level", "Top level"),
    ("schema.actions.remove", "Remove"),
    ("schema.kind.column", "Column"),
    ("schema.kind.group", "Group"),
    ("schema.kind.text", "Text"),
    ("schema.kind.boolean", "Yes / no"),
    ("schema.kind.integer", "Integer"),
    ("schema.kind.decimal", "Decimal"),
    ("schema.kind.date", "Date"),
    ("schema.kind.datetime", "Date and time"),
    ("schema.kind.enum", "Choice"),
    ("schema.kind.attachment", "Attachment"),
    ("schema.kind.geometry", "Geometry"),
    ("schema.kind.section", "Section"),
    ("schema.kind.note", "Note"),
    ("schema.add.title", "Add an element"),
    ("schema.add.inside", "Add inside this group"),
    ("schema.add.inside-section", "Add inside this section"),
    (
        "schema.add.lead",
        "Select an element in the structure to edit it, or add one here.",
    ),
    ("schema.add.what", "Kind"),
    ("schema.add.submit", "Add"),
    ("schema.detail.column", "Column: {$label}"),
    ("schema.detail.group", "Group: {$label}"),
    ("schema.detail.section", "Section: {$label}"),
    ("schema.detail.note", "Note"),
    ("schema.type", "Type"),
    ("schema.unit", "Unit"),
    ("schema.unit.none", "No unit"),
    ("schema.arity", "Values"),
    ("schema.arity.one", "One"),
    ("schema.arity.many", "Many"),
    ("schema.cardinality", "Rows"),
    ("schema.cardinality.one", "One"),
    ("schema.cardinality.many", "Many"),
    ("schema.audience", "Audience"),
    ("schema.required", "Required"),
    ("schema.format", "Format"),
    ("schema.format.none", "Free text"),
    ("schema.format.email", "Email address"),
    ("schema.format.phone", "Phone number"),
    ("schema.format.iban", "IBAN"),
    ("schema.format.regex", "Custom pattern"),
    ("schema.format.pattern", "Pattern"),
    (
        "schema.format.pattern.help",
        "A full-match regular expression without backtracking (e.g. [0-9]{5}).",
    ),
    (
        "schema.error.pattern-required",
        "A pattern is required for a custom format.",
    ),
    ("schema.audience.all", "Everyone"),
    ("schema.audience.reviewer", "Reviewers only"),
    ("schema.section.help", "Help text"),
    ("schema.note.body", "Text"),
    ("schema.options", "Options"),
    (
        "schema.options.help",
        "Options are saved as you edit them. A choice needs at least one.",
    ),
    ("schema.options.label", "Option"),
    ("schema.options.empty", "No options yet."),
    ("schema.options.new", "New option"),
    ("schema.options.add", "Add option"),
    ("schema.options.remove", "Remove option {$label}"),
    ("schema.options.default", "Option 1"),
    (
        "schema.notice.option-added",
        "Added the option \u{201c}{$label}\u{201d}.",
    ),
    (
        "schema.notice.option-removed",
        "Removed the option \u{201c}{$label}\u{201d}.",
    ),
    ("schema.attachment.accept", "Accepted types"),
    (
        "schema.attachment.accept.help",
        "Media types separated by commas (application/pdf, image/*); empty accepts everything.",
    ),
    ("schema.attachment.max-bytes", "Maximum size (bytes)"),
    ("schema.save", "Save"),
    ("schema.status.saving", "Saving\u{2026}"),
    ("schema.status.saved", "Saved your changes."),
    (
        "schema.notice.column-added",
        "Added the column \u{201c}{$label}\u{201d}.",
    ),
    (
        "schema.notice.group-added",
        "Added the group \u{201c}{$label}\u{201d}.",
    ),
    (
        "schema.notice.section-added",
        "Added the section \u{201c}{$label}\u{201d}.",
    ),
    ("schema.notice.note-added", "Added the note."),
    ("schema.notice.saved", "Saved your changes."),
    ("schema.notice.moved", "Moved \u{201c}{$label}\u{201d}."),
    ("schema.notice.removed", "Removed \u{201c}{$label}\u{201d}."),
    ("schema.notice.discarded", "The draft has been discarded."),
    ("schema.error.label-required", "A label is required."),
    ("schema.error.title-required", "A title is required."),
    ("schema.error.body-required", "A text is required."),
    (
        "schema.error.options-required",
        "A choice needs at least one option.",
    ),
    ("schema.error.unit", "Unknown unit."),
    (
        "schema.error.max-bytes",
        "The maximum size must be a positive number.",
    ),
    (
        "schema.error.conflict",
        "The draft changed in the meantime. Reload the page and try again.",
    ),
    ("schema.error.refused", "The change was refused: {$reason}"),
    ("form.title", "Title"),
    ("form.description", "Description"),
    (
        "organization.procedures.title",
        "Procedures ({$count :integer})",
    ),
    ("organization.procedures.empty", "No procedures yet."),
    ("settings.tab.account", "Account"),
    ("settings.tab.security", "Security"),
    ("settings.account.profile.title", "User profile"),
    ("settings.account.profile.save", "Save changes"),
    (
        "settings.account.profile.error.name-required",
        "Please enter a name.",
    ),
    ("settings.account.email.title", "Email address"),
    ("settings.security.sessions.title", "Active sessions"),
    ("settings.security.sessions.current", "Current session"),
    ("settings.security.sessions.revoke", "Revoke session"),
    ("settings.security.sessions.unknown", "Unknown"),
    (
        "settings.security.sessions.unknown-browser",
        "Unknown browser",
    ),
    (
        "settings.security.sessions.created",
        "Signed in on {$date :date style=medium}",
    ),
    (
        "settings.security.sessions.expires",
        "Expires on {$date :date style=medium}",
    ),
    ("settings.security.tokens.title", "API tokens"),
    (
        "settings.security.tokens.description",
        "Tokens authenticate API clients as your account. Each token expires {$months} months after it is created.",
    ),
    ("settings.security.tokens.name", "Token name"),
    ("settings.security.tokens.create", "Create token"),
    (
        "settings.security.tokens.error.name-required",
        "Please enter a token name.",
    ),
    (
        "settings.security.tokens.issued.title",
        "Token “{$name}” created",
    ),
    (
        "settings.security.tokens.issued.hint",
        "Copy it now: it will not be shown again.",
    ),
    ("settings.security.tokens.empty", "No API tokens."),
    ("settings.security.tokens.revoke", "Revoke"),
    // The per-row button's accessible name (`aria-label`): keeps the
    // visible "Revoke" as its head (WCAG 2.5.3) and names the token.
    (
        "settings.security.tokens.revoke-named",
        "Revoke token {$name}",
    ),
    (
        "settings.security.tokens.created",
        "Created on {$date :date style=medium}",
    ),
    (
        "settings.security.tokens.expires",
        "Expires on {$date :date style=medium}",
    ),
    ("error.not-found", "Page not found."),
];

/// French messages.
pub const FR: &[(&str, &str)] = &[
    ("app.title", "Varve"),
    ("nav.sign-in", "Se connecter"),
    ("nav.sign-up", "Créer un compte"),
    ("nav.sign-out", "Se déconnecter"),
    // See the English table: the icon-only trigger's `aria-label`.
    ("nav.account-menu", "Menu du compte"),
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
    ("form.language", "Langue"),
    // Endonyms, identical to the English table — see its note.
    ("locale.en", "English"),
    ("locale.fr", "Français"),
    ("settings.title", "Paramètres"),
    ("organizations.title", "Organisations"),
    ("organizations.list.title", "Vos organisations"),
    (
        "organizations.list.empty",
        "Vous n'êtes membre d'aucune organisation pour le moment.",
    ),
    ("organizations.create.title", "Créer une organisation"),
    ("organizations.create.submit", "Créer l'organisation"),
    (
        "organizations.create.error.name-required",
        "Veuillez saisir un nom.",
    ),
    (
        "organizations.create.error.slug-required",
        "Veuillez saisir un identifiant.",
    ),
    (
        "organizations.create.error.slug-invalid",
        "L'identifiant ne peut contenir que des lettres minuscules, des chiffres et des tirets.",
    ),
    (
        "organizations.create.error.slug-taken",
        "Cet identifiant est déjà utilisé.",
    ),
    ("form.slug", "Identifiant"),
    ("form.slug.hint", "mon-organisation"),
    (
        "organization.created",
        "Créée le {$date :date style=medium}",
    ),
    ("organization.members.title", "Membres ({$count :integer})"),
    ("organization.teams.title", "Équipes ({$count :integer})"),
    ("organization.teams.empty", "Aucune équipe pour le moment."),
    ("organization.teams.manage", "Gérer les équipes"),
    ("teams.title", "Équipes"),
    ("teams.lead", "Équipes de"),
    ("teams.list.title", "Équipes"),
    ("teams.list.empty", "Aucune équipe pour le moment."),
    ("teams.create.title", "Créer une équipe"),
    ("teams.create.submit", "Créer l'équipe"),
    (
        "teams.create.error.name-required",
        "Veuillez saisir un nom.",
    ),
    ("organization.procedures.manage", "Gérer les procédures"),
    ("procedures.title", "Procédures"),
    ("procedures.lead", "Procédures de"),
    ("procedures.list.title", "Procédures"),
    ("procedures.list.empty", "Aucune procédure pour le moment."),
    ("procedures.create.title", "Créer une procédure"),
    ("procedures.create.submit", "Créer la procédure"),
    (
        "procedures.create.error.title-required",
        "Veuillez saisir un titre.",
    ),
    ("form.label", "Libellé"),
    ("procedure.lead", "Une procédure de"),
    ("procedure.draft.title", "Prochaine révision"),
    ("procedure.draft.badge", "Brouillon"),
    (
        "procedure.draft.none",
        "Pas encore de brouillon. L'éditeur de schéma en ouvre un.",
    ),
    (
        "procedure.draft.summary",
        ".input {$columns :integer}\n\
         .input {$groups :integer}\n\
         .match $columns $groups\n\
         one one {{{$columns} colonne et {$groups} groupe, enregistré le {$date :date style=long}.}}\n\
         one * {{{$columns} colonne et {$groups} groupes, enregistré le {$date :date style=long}.}}\n\
         * one {{{$columns} colonnes et {$groups} groupe, enregistré le {$date :date style=long}.}}\n\
         * * {{{$columns} colonnes et {$groups} groupes, enregistré le {$date :date style=long}.}}",
    ),
    ("procedure.draft.edit", "Modifier le schéma"),
    ("schema.title", "Schéma de {$procedure}"),
    ("schema.back", "Retour à la procédure"),
    ("schema.tab.editor", "Édition"),
    ("schema.tab.preview", "Aperçu"),
    (
        "schema.preview.empty",
        "Rien à prévisualiser pour le moment. Ajoutez des éléments dans l'éditeur.",
    ),
    (
        "schema.preview.geometry",
        "Saisie sur carte — non affichée dans l'aperçu.",
    ),
    (
        "schema.state.none",
        "Pas encore de brouillon\u{a0}: ajouter un élément en ouvre un.",
    ),
    (
        "schema.state.draft",
        ".input {$columns :integer}\n\
         .input {$groups :integer}\n\
         .match $columns $groups\n\
         one one {{{$columns} colonne, {$groups} groupe \u{2014} enregistré le {$date :date style=long}.}}\n\
         one * {{{$columns} colonne, {$groups} groupes \u{2014} enregistré le {$date :date style=long}.}}\n\
         * one {{{$columns} colonnes, {$groups} groupe \u{2014} enregistré le {$date :date style=long}.}}\n\
         * * {{{$columns} colonnes, {$groups} groupes \u{2014} enregistré le {$date :date style=long}.}}",
    ),
    ("schema.discard", "Abandonner le brouillon"),
    (
        "schema.discard.question",
        "Abandonner tout le brouillon\u{a0}? Toutes les modifications non publiées seront perdues.",
    ),
    ("schema.discard.confirm", "Abandonner"),
    ("schema.discard.keep", "Continuer l'édition"),
    ("schema.structure.title", "Structure"),
    (
        "schema.structure.empty",
        "Le schéma est vide. Ajoutez une colonne ou un groupe pour commencer.",
    ),
    ("schema.actions", "Actions"),
    ("schema.actions.for", "Actions pour {$label}"),
    ("schema.actions.move-up", "Monter"),
    ("schema.actions.move-down", "Descendre"),
    ("schema.actions.move-top", "Placer en premier"),
    ("schema.actions.move-bottom", "Placer en dernier"),
    ("schema.actions.move-after", "Placer après"),
    ("schema.actions.move-to", "Déplacer vers"),
    ("schema.actions.top-level", "Premier niveau"),
    ("schema.actions.remove", "Supprimer"),
    ("schema.kind.column", "Colonne"),
    ("schema.kind.group", "Groupe"),
    ("schema.kind.text", "Texte"),
    ("schema.kind.boolean", "Oui / non"),
    ("schema.kind.integer", "Nombre entier"),
    ("schema.kind.decimal", "Nombre décimal"),
    ("schema.kind.date", "Date"),
    ("schema.kind.datetime", "Date et heure"),
    ("schema.kind.enum", "Choix"),
    ("schema.kind.attachment", "Pièce jointe"),
    ("schema.kind.geometry", "Géométrie"),
    ("schema.kind.section", "Section"),
    ("schema.kind.note", "Note"),
    ("schema.add.title", "Ajouter un élément"),
    ("schema.add.inside", "Ajouter dans ce groupe"),
    ("schema.add.inside-section", "Ajouter dans cette section"),
    (
        "schema.add.lead",
        "Sélectionnez un élément dans la structure pour le modifier, ou ajoutez-en un ici.",
    ),
    ("schema.add.what", "Nature"),
    ("schema.add.submit", "Ajouter"),
    ("schema.detail.column", "Colonne\u{a0}: {$label}"),
    ("schema.detail.group", "Groupe\u{a0}: {$label}"),
    ("schema.detail.section", "Section\u{a0}: {$label}"),
    ("schema.detail.note", "Note"),
    ("schema.type", "Type"),
    ("schema.unit", "Unité"),
    ("schema.unit.none", "Sans unité"),
    ("schema.arity", "Valeurs"),
    ("schema.arity.one", "Une"),
    ("schema.arity.many", "Plusieurs"),
    ("schema.cardinality", "Lignes"),
    ("schema.cardinality.one", "Une"),
    ("schema.cardinality.many", "Plusieurs"),
    ("schema.audience", "Visibilité"),
    ("schema.required", "Obligatoire"),
    ("schema.format", "Format"),
    ("schema.format.none", "Texte libre"),
    ("schema.format.email", "Adresse électronique"),
    ("schema.format.phone", "Numéro de téléphone"),
    ("schema.format.iban", "IBAN"),
    ("schema.format.regex", "Motif personnalisé"),
    ("schema.format.pattern", "Motif"),
    (
        "schema.format.pattern.help",
        "Expression régulière en correspondance totale, sans retour arrière (ex.\u{a0}[0-9]{5}).",
    ),
    (
        "schema.error.pattern-required",
        "Un motif est requis pour un format personnalisé.",
    ),
    ("schema.audience.all", "Tout le monde"),
    ("schema.audience.reviewer", "Instructeurs uniquement"),
    ("schema.section.help", "Texte d'aide"),
    ("schema.note.body", "Texte"),
    ("schema.options", "Options"),
    (
        "schema.options.help",
        "Les options sont enregistrées au fil de la saisie. Un choix a besoin d'au moins une option.",
    ),
    ("schema.options.label", "Option"),
    ("schema.options.empty", "Aucune option pour le moment."),
    ("schema.options.new", "Nouvelle option"),
    ("schema.options.add", "Ajouter l'option"),
    ("schema.options.remove", "Supprimer l'option {$label}"),
    ("schema.options.default", "Option 1"),
    (
        "schema.notice.option-added",
        "L'option «\u{a0}{$label}\u{a0}» a été ajoutée.",
    ),
    (
        "schema.notice.option-removed",
        "L'option «\u{a0}{$label}\u{a0}» a été supprimée.",
    ),
    ("schema.attachment.accept", "Types acceptés"),
    (
        "schema.attachment.accept.help",
        "Types de média séparés par des virgules (application/pdf, image/*)\u{a0}; vide accepte tout.",
    ),
    ("schema.attachment.max-bytes", "Taille maximale (octets)"),
    ("schema.save", "Enregistrer"),
    ("schema.status.saving", "Enregistrement\u{2026}"),
    (
        "schema.status.saved",
        "Vos modifications sont enregistrées.",
    ),
    (
        "schema.notice.column-added",
        "La colonne «\u{a0}{$label}\u{a0}» a été ajoutée.",
    ),
    (
        "schema.notice.group-added",
        "Le groupe «\u{a0}{$label}\u{a0}» a été ajouté.",
    ),
    (
        "schema.notice.section-added",
        "La section «\u{a0}{$label}\u{a0}» a été ajoutée.",
    ),
    ("schema.notice.note-added", "La note a été ajoutée."),
    (
        "schema.notice.saved",
        "Vos modifications sont enregistrées.",
    ),
    (
        "schema.notice.moved",
        "«\u{a0}{$label}\u{a0}» a été déplacé.",
    ),
    (
        "schema.notice.removed",
        "«\u{a0}{$label}\u{a0}» a été supprimé.",
    ),
    ("schema.notice.discarded", "Le brouillon a été abandonné."),
    ("schema.error.label-required", "Un libellé est requis."),
    ("schema.error.title-required", "Un titre est requis."),
    ("schema.error.body-required", "Un texte est requis."),
    (
        "schema.error.options-required",
        "Un choix a besoin d'au moins une option.",
    ),
    ("schema.error.unit", "Unité inconnue."),
    (
        "schema.error.max-bytes",
        "La taille maximale doit être un nombre positif.",
    ),
    (
        "schema.error.conflict",
        "Le brouillon a changé entre-temps. Rechargez la page et réessayez.",
    ),
    (
        "schema.error.refused",
        "La modification a été refusée\u{a0}: {$reason}",
    ),
    ("form.title", "Titre"),
    ("form.description", "Description"),
    (
        "organization.procedures.title",
        "Procédures ({$count :integer})",
    ),
    (
        "organization.procedures.empty",
        "Aucune procédure pour le moment.",
    ),
    ("settings.tab.account", "Compte"),
    ("settings.tab.security", "Sécurité"),
    ("settings.account.profile.title", "Profil de l'utilisateur"),
    (
        "settings.account.profile.save",
        "Enregistrer les modifications",
    ),
    (
        "settings.account.profile.error.name-required",
        "Veuillez saisir un nom.",
    ),
    ("settings.account.email.title", "Adresse électronique"),
    ("settings.security.sessions.title", "Sessions actives"),
    ("settings.security.sessions.current", "Session actuelle"),
    ("settings.security.sessions.revoke", "Révoquer la session"),
    ("settings.security.sessions.unknown", "Inconnu"),
    (
        "settings.security.sessions.unknown-browser",
        "Navigateur inconnu",
    ),
    (
        "settings.security.sessions.created",
        "Ouverte le {$date :date style=medium}",
    ),
    (
        "settings.security.sessions.expires",
        "Expire le {$date :date style=medium}",
    ),
    ("settings.security.tokens.title", "Jetons d’API"),
    (
        "settings.security.tokens.description",
        "Les jetons authentifient les clients de l’API au nom de votre compte. Chaque jeton expire {$months} mois après sa création.",
    ),
    ("settings.security.tokens.name", "Nom du jeton"),
    ("settings.security.tokens.create", "Créer un jeton"),
    (
        "settings.security.tokens.error.name-required",
        "Veuillez saisir un nom de jeton.",
    ),
    (
        "settings.security.tokens.issued.title",
        "Jeton «\u{a0}{$name}\u{a0}» créé",
    ),
    (
        "settings.security.tokens.issued.hint",
        "Copiez-le maintenant\u{a0}: il ne sera plus affiché.",
    ),
    ("settings.security.tokens.empty", "Aucun jeton d’API."),
    ("settings.security.tokens.revoke", "Révoquer"),
    // See the English table: the per-row button's `aria-label`.
    (
        "settings.security.tokens.revoke-named",
        "Révoquer le jeton {$name}",
    ),
    (
        "settings.security.tokens.created",
        "Créé le {$date :date style=medium}",
    ),
    (
        "settings.security.tokens.expires",
        "Expire le {$date :date style=medium}",
    ),
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
