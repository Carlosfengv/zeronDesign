use crate::templates::BuildOverlayRequest;

pub(super) fn render_index(request: &BuildOverlayRequest) -> String {
    let title = request
        .content_hierarchy
        .first()
        .cloned()
        .unwrap_or_else(|| "AnyDesign Runtime".to_string());
    let hierarchy = request
        .content_hierarchy
        .iter()
        .enumerate()
        .map(|(index, item)| {
            format!(
                "<article class=\"deco-card group\">\n          <span class=\"deco-step\">{}</span>\n          <h3>{}</h3>\n          <p>{}</p>\n        </article>",
                roman_numeral(index),
                escape_html(item),
                escape_html(&request.visual_direction)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "---\nimport '../styles/global.css';\nconst audience = {:?};\n---\n<html lang=\"en\">\n  <head>\n    <meta charset=\"utf-8\" />\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <title>{}</title>\n    <link rel=\"preconnect\" href=\"https://fonts.googleapis.com\" />\n    <link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin />\n    <link href=\"https://fonts.googleapis.com/css2?family=Josefin+Sans:wght@400;500;600;700&family=Marcellus&display=swap\" rel=\"stylesheet\" />\n  </head>\n  <body class=\"min-h-screen bg-[#0A0A0A] text-[#F2F0E4]\">\n    <main class=\"deco-shell\">\n      <section class=\"deco-hero\" aria-labelledby=\"page-title\">\n        <div class=\"deco-sunburst\" aria-hidden=\"true\"></div>\n        <p class=\"deco-kicker\">astro-website</p>\n        <h1 id=\"page-title\">{}</h1>\n        <p class=\"deco-lede\">{}</p>\n        <p class=\"deco-audience\">Audience: {{audience}}</p>\n        <div class=\"deco-actions\" aria-label=\"Primary actions\">\n          <a class=\"deco-button deco-button-solid\" href=\"#system\">View System</a>\n          <a class=\"deco-button deco-button-outline\" href=\"#components\">Components</a>\n        </div>\n      </section>\n\n      <section id=\"system\" class=\"deco-section\" aria-labelledby=\"system-title\">\n        <div class=\"deco-section-heading\">\n          <span aria-hidden=\"true\"></span>\n          <h2 id=\"system-title\">Design System</h2>\n          <span aria-hidden=\"true\"></span>\n        </div>\n        <div class=\"deco-grid\">\n          {}\n        </div>\n      </section>\n\n      <section id=\"components\" class=\"deco-section deco-component-band\" aria-labelledby=\"components-title\">\n        <div class=\"deco-section-heading\">\n          <span aria-hidden=\"true\"></span>\n          <h2 id=\"components-title\">Component Language</h2>\n          <span aria-hidden=\"true\"></span>\n        </div>\n        <div class=\"deco-component-grid\">\n          <div class=\"deco-card deco-card-feature\">\n            <span class=\"deco-diamond\" aria-hidden=\"true\"><span></span></span>\n            <h3>Buttons</h3>\n            <p>Sharp corners, gold borders, theatrical hover glow, and all-caps precision.</p>\n          </div>\n          <div class=\"deco-card deco-card-feature\">\n            <span class=\"deco-diamond\" aria-hidden=\"true\"><span></span></span>\n            <h3>Cards</h3>\n            <p>Double frames, stepped corner brackets, charcoal panels, and measured ornament.</p>\n          </div>\n          <div class=\"deco-card deco-card-feature\">\n            <span class=\"deco-diamond\" aria-hidden=\"true\"><span></span></span>\n            <h3>Inputs</h3>\n            <p>Transparent fields, gold underlines, champagne text, and mechanical focus states.</p>\n          </div>\n        </div>\n      </section>\n    </main>\n  </body>\n</html>\n",
        request.audience,
        escape_html(&title),
        escape_html(&title),
        escape_html(&request.visual_direction),
        hierarchy,
    )
}

pub(super) fn render_global_css() -> &'static str {
    r#"@import "tailwindcss";

:root {
  --deco-obsidian: #0A0A0A;
  --deco-champagne: #F2F0E4;
  --deco-charcoal: #141414;
  --deco-gold: #D4AF37;
  --deco-blue: #1E3D59;
  --deco-pewter: #888888;
  --deco-gold-glow: rgba(212, 175, 55, 0.28);
  color-scheme: dark;
  font-family: "Josefin Sans", ui-sans-serif, system-ui, sans-serif;
}

* {
  box-sizing: border-box;
}

html {
  background: var(--deco-obsidian);
}

body {
  margin: 0;
  min-height: 100vh;
  background:
    radial-gradient(circle at 50% 14%, rgba(212, 175, 55, 0.18), transparent 28rem),
    repeating-linear-gradient(45deg, rgba(212, 175, 55, 0.045) 0 1px, transparent 1px 28px),
    repeating-linear-gradient(-45deg, rgba(212, 175, 55, 0.035) 0 1px, transparent 1px 28px),
    var(--deco-obsidian);
  color: var(--deco-champagne);
}

a {
  color: inherit;
}

.deco-shell {
  position: relative;
  isolation: isolate;
  width: min(100%, 1280px);
  margin: 0 auto;
  padding: 32px clamp(16px, 4vw, 48px) 72px;
}

.deco-shell::before,
.deco-shell::after {
  content: "";
  position: fixed;
  top: 0;
  bottom: 0;
  width: 1px;
  background: linear-gradient(transparent, rgba(212, 175, 55, 0.55), transparent);
  pointer-events: none;
}

.deco-shell::before {
  left: clamp(16px, 5vw, 72px);
}

.deco-shell::after {
  right: clamp(16px, 5vw, 72px);
}

.deco-hero {
  position: relative;
  display: grid;
  min-height: 78vh;
  place-items: center;
  overflow: hidden;
  border: 3px double rgba(212, 175, 55, 0.76);
  background:
    linear-gradient(180deg, rgba(20, 20, 20, 0.82), rgba(10, 10, 10, 0.94)),
    radial-gradient(circle at center, rgba(212, 175, 55, 0.16), transparent 48%);
  clip-path: polygon(0 24px, 24px 24px, 24px 0, calc(100% - 24px) 0, calc(100% - 24px) 24px, 100% 24px, 100% calc(100% - 24px), calc(100% - 24px) calc(100% - 24px), calc(100% - 24px) 100%, 24px 100%, 24px calc(100% - 24px), 0 calc(100% - 24px));
  padding: clamp(64px, 12vw, 128px) clamp(20px, 6vw, 80px);
  text-align: center;
}

.deco-hero > * {
  position: relative;
  z-index: 1;
}

.deco-sunburst {
  position: absolute;
  inset: -18%;
  opacity: 0.42;
  background:
    repeating-conic-gradient(from -6deg at 50% 50%, rgba(212, 175, 55, 0.28) 0deg 3deg, transparent 3deg 12deg);
  mask-image: radial-gradient(circle at center, black, transparent 62%);
  pointer-events: none;
}

.deco-kicker,
.deco-audience {
  margin: 0;
  color: var(--deco-gold);
  font-size: 0.78rem;
  font-weight: 700;
  letter-spacing: 0.28em;
  text-transform: uppercase;
}

.deco-hero h1 {
  max-width: 920px;
  margin: 24px 0 0;
  color: var(--deco-gold);
  font-family: "Marcellus", Georgia, serif;
  font-size: clamp(3.1rem, 9vw, 7.2rem);
  font-weight: 400;
  letter-spacing: 0.18em;
  line-height: 0.92;
  text-transform: uppercase;
  text-shadow: 0 0 26px rgba(212, 175, 55, 0.22);
}

.deco-lede {
  max-width: 760px;
  margin: 32px auto 0;
  color: var(--deco-champagne);
  font-size: clamp(1.05rem, 2vw, 1.3rem);
  line-height: 1.75;
}

.deco-audience {
  margin-top: 24px;
  color: var(--deco-pewter);
}

.deco-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 16px;
  margin-top: 36px;
}

.deco-button {
  display: inline-flex;
  min-height: 48px;
  align-items: center;
  justify-content: center;
  border: 2px solid var(--deco-gold);
  padding: 0 24px;
  color: var(--deco-gold);
  font-size: 0.8rem;
  font-weight: 700;
  letter-spacing: 0.2em;
  text-decoration: none;
  text-transform: uppercase;
  transition: background-color 320ms ease, box-shadow 320ms ease, color 320ms ease, transform 320ms ease;
}

.deco-button:hover,
.deco-button:focus-visible {
  box-shadow: 0 0 24px rgba(212, 175, 55, 0.42);
  transform: translateY(-2px);
}

.deco-button-solid {
  background: linear-gradient(135deg, #D4AF37, #F2E8C4 50%, #B48924);
  color: var(--deco-obsidian);
}

.deco-button-outline:hover,
.deco-button-outline:focus-visible {
  background: var(--deco-gold);
  color: var(--deco-obsidian);
}

.deco-section {
  padding: clamp(72px, 11vw, 128px) 0 0;
}

.deco-section-heading {
  display: grid;
  grid-template-columns: minmax(48px, 96px) auto minmax(48px, 96px);
  align-items: center;
  justify-content: center;
  gap: 20px;
  margin-bottom: 40px;
  text-align: center;
}

.deco-section-heading span {
  height: 1px;
  background: var(--deco-gold);
  box-shadow: 0 0 12px var(--deco-gold-glow);
}

.deco-section-heading h2 {
  margin: 0;
  color: var(--deco-gold);
  font-family: "Marcellus", Georgia, serif;
  font-size: clamp(1.6rem, 4vw, 3rem);
  font-weight: 400;
  letter-spacing: 0.2em;
  text-transform: uppercase;
}

.deco-grid,
.deco-component-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 24px;
}

.deco-card {
  position: relative;
  min-height: 220px;
  overflow: hidden;
  border: 1px solid rgba(212, 175, 55, 0.34);
  background:
    linear-gradient(180deg, rgba(20, 20, 20, 0.96), rgba(10, 10, 10, 0.96)),
    var(--deco-charcoal);
  padding: 28px;
  transition: border-color 420ms ease, box-shadow 420ms ease, transform 420ms ease;
}

.deco-card::before,
.deco-card::after {
  content: "";
  position: absolute;
  width: 34px;
  height: 34px;
  opacity: 0.74;
  transition: opacity 420ms ease;
}

.deco-card::before {
  top: 8px;
  left: 8px;
  border-top: 2px solid var(--deco-gold);
  border-left: 2px solid var(--deco-gold);
}

.deco-card::after {
  right: 8px;
  bottom: 8px;
  border-right: 2px solid var(--deco-gold);
  border-bottom: 2px solid var(--deco-gold);
}

.deco-card:hover {
  border-color: var(--deco-gold);
  box-shadow: 0 0 22px rgba(212, 175, 55, 0.22);
  transform: translateY(-8px);
}

.deco-card:hover::before,
.deco-card:hover::after {
  opacity: 1;
}

.deco-step {
  display: inline-flex;
  margin-bottom: 24px;
  color: var(--deco-gold);
  font-family: "Marcellus", Georgia, serif;
  font-size: 0.88rem;
  letter-spacing: 0.22em;
}

.deco-card h3 {
  margin: 0;
  color: var(--deco-gold);
  font-family: "Marcellus", Georgia, serif;
  font-size: 1.4rem;
  font-weight: 400;
  letter-spacing: 0.16em;
  line-height: 1.25;
  text-transform: uppercase;
}

.deco-card p {
  margin: 18px 0 0;
  color: var(--deco-pewter);
  font-size: 1rem;
  line-height: 1.7;
}

.deco-component-band {
  padding-bottom: 32px;
}

.deco-card-feature {
  min-height: 260px;
  text-align: center;
}

.deco-diamond {
  display: inline-grid;
  width: 56px;
  height: 56px;
  place-items: center;
  margin-bottom: 28px;
  border: 2px solid var(--deco-gold);
  transform: rotate(45deg);
}

.deco-diamond span {
  display: block;
  width: 22px;
  height: 22px;
  border: 1px solid var(--deco-gold);
  background: rgba(212, 175, 55, 0.16);
  transform: rotate(0deg);
}

@media (max-width: 900px) {
  .deco-grid,
  .deco-component-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 640px) {
  .deco-shell {
    padding-inline: 16px;
  }

  .deco-hero {
    min-height: 72vh;
  }

  .deco-grid,
  .deco-component-grid {
    grid-template-columns: 1fr;
  }

  .deco-section-heading {
    grid-template-columns: 48px auto 48px;
    gap: 12px;
  }
}
"#
}

fn roman_numeral(index: usize) -> &'static str {
    const NUMERALS: [&str; 12] = [
        "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII",
    ];
    NUMERALS.get(index).copied().unwrap_or("XII")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
