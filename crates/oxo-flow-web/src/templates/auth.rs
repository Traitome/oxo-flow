//! Authentication templates for oxo-flow-web.
//!
//! Provides login page and authentication-related partials.

use maud::{DOCTYPE, Markup, html};

use super::partials::theme;

/// Login page template.
pub fn login_page(error: Option<&str>) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title { "Sign In — oxo-flow" }

                // HTMX for form submission
                script src="https://unpkg.com/htmx.org@1.9.10/dist/htmx.min.js" {}

                // Styles
                style {
                    (login_styles())
                }
            }
            body {
                div class="login-container" {
                    // Login card
                    div class="login-card" {
                        // Logo
                        div class="login-header" {
                            h1 { "oxo-" span { "flow" } }
                            p { "Command Center" }
                        }

                        // Login form
                        form
                            hx-post="/api/auth/login"
                            hx-target="#login-result"
                            hx-swap="innerHTML"
                        {
                            // Username field
                            div class="form-group" {
                                label for="username" { "Username" }
                                input
                                    type="text"
                                    id="username"
                                    name="username"
                                    placeholder="Enter your username"
                                    required
                                    autofocus
                                {}
                            }

                            // Password field
                            div class="form-group" {
                                label for="password" { "Password" }
                                input
                                    type="password"
                                    id="password"
                                    name="password"
                                    placeholder="Enter your password"
                                    required
                                {}
                            }

                            // Error message
                            @if let Some(err) = error {
                                div id="login-error" class="login-error" {
                                    (err)
                                }
                            }

                            // HTMX result area
                            div id="login-result" {}

                            // Submit button
                            button
                                type="submit"
                                class="btn btn-primary login-btn"
                            { "Sign In" }
                        }

                        // Footer info
                        div class="login-footer" {
                            p { "Default credentials: admin/admin" }
                            p class="hint" { "Contact your administrator for access" }
                        }
                    }

                    // Success redirect (handled by HTMX HX-Redirect header)
                    div id="login-success" class="hidden" {}
                }
            }
        }
    }
}

/// Login result partial for HTMX response.
pub fn login_result_partial(success: bool, message: &str, _token: Option<&str>) -> Markup {
    if success {
        html! {
            div class="login-success" {
                p { (message) }
                // HTMX will handle redirect via HX-Redirect header
            }
        }
    } else {
        html! {
            div class="login-error" {
                p { (message) }
            }
        }
    }
}

/// Logout confirmation partial.
pub fn logout_partial() -> Markup {
    html! {
        div class="logout-confirm" {
            h3 { "Sign Out" }
            p { "Are you sure you want to sign out?" }
            div class="logout-actions" {
                button
                    class="btn btn-primary"
                    hx-post="/api/auth/logout"
                    hx-target="#user-info"
                    hx-swap="outerHTML"
                { "Sign Out" }
                button class="btn btn-outline" onclick="closeLogoutModal()" { "Cancel" }
            }
        }
    }
}

/// User info partial shown in sidebar.
pub fn user_info_partial(username: &str, role: &str) -> Markup {
    html! {
        div class="user-info" id="user-info" {
            div class="avatar" title=(username) {
                (username.chars().next().unwrap_or('G').to_uppercase())
            }
            span class="user-name" { (username) }
            span class="user-role" { (role) }
            span class="status-dot ok" {}
            button
                class="logout-btn"
                onclick="showLogoutModal()"
            {
                (maud::PreEscaped("&#10140;")) // arrow icon
            }
        }
    }
}

/// Login styles specific to the auth page.
fn login_styles() -> String {
    format!(
        r#"
:root {{
    --bg: {bg};
    --surface: {surface};
    --border: {border};
    --accent: {accent};
    --text: {text};
    --text-muted: {text_muted};
    --error: {error};
    --success: {success};
    --radius: 6px;
    --font: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
}}
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
body {{
    font-family: var(--font);
    background: var(--bg);
    color: var(--text);
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
}}
.login-container {{
    width: 100%;
    max-width: 400px;
    padding: 2rem;
}}
.login-card {{
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 2rem;
}}
.login-header {{
    text-align: center;
    margin-bottom: 2rem;
}}
.login-header h1 {{
    font-size: 2rem;
    font-weight: 700;
    letter-spacing: -0.02em;
}}
.login-header h1 span {{
    color: var(--accent);
}}
.login-header p {{
    color: var(--text-muted);
    font-size: 0.85rem;
    margin-top: 0.25rem;
}}
.form-group {{
    margin-bottom: 1.25rem;
}}
.form-group label {{
    display: block;
    font-size: 0.85rem;
    color: var(--text-muted);
    margin-bottom: 0.5rem;
}}
.form-group input {{
    width: 100%;
    padding: 0.75rem 1rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-size: 1rem;
    outline: none;
}}
.form-group input:focus {{
    border-color: var(--accent);
}}
.login-error {{
    background: rgba(248, 81, 73, 0.1);
    border: 1px solid var(--error);
    border-radius: var(--radius);
    padding: 0.75rem;
    margin-bottom: 1rem;
    color: var(--error);
    font-size: 0.85rem;
}}
.login-success {{
    background: rgba(63, 185, 80, 0.1);
    border: 1px solid var(--success);
    border-radius: var(--radius);
    padding: 0.75rem;
    margin-bottom: 1rem;
    color: var(--success);
    font-size: 0.85rem;
}}
.btn {{
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    padding: 0.75rem 1.5rem;
    border-radius: var(--radius);
    border: none;
    cursor: pointer;
    font-size: 1rem;
    font-weight: 500;
    font-family: var(--font);
    transition: all 0.15s;
}}
.btn-primary {{
    background: var(--accent);
    color: var(--bg);
}}
.btn-primary:hover {{
    filter: brightness(1.15);
}}
.btn-outline {{
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text);
}}
.btn-outline:hover {{
    background: rgba(139, 148, 158, 0.1);
}}
.login-btn {{
    width: 100%;
    margin-top: 0.5rem;
}}
.login-footer {{
    text-align: center;
    margin-top: 1.5rem;
    padding-top: 1rem;
    border-top: 1px solid var(--border);
}}
.login-footer p {{
    color: var(--text-muted);
    font-size: 0.78rem;
}}
.login-footer .hint {{
    font-size: 0.72rem;
    opacity: 0.7;
}}
.hidden {{
    display: none;
}}
.logout-confirm {{
    text-align: center;
}}
.logout-confirm h3 {{
    margin-bottom: 0.5rem;
}}
.logout-confirm p {{
    color: var(--text-muted);
    margin-bottom: 1rem;
}}
.logout-actions {{
    display: flex;
    gap: 0.5rem;
    justify-content: center;
}}
"#,
        bg = theme::BG,
        surface = theme::SURFACE,
        border = theme::BORDER,
        accent = theme::ACCENT,
        text = theme::TEXT,
        text_muted = theme::TEXT_MUTED,
        error = theme::ERROR,
        success = theme::SUCCESS,
    )
}
