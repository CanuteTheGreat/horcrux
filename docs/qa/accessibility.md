# Accessibility gate (axe-core)

CI step "Accessibility gate" in the UI E2E job runs `horcrux-api/horcrux-ui/e2e/a11y_e2e.py` against the live server
with the built UI: ten key pages x desktop (1280) and phone (390) x light and dark.

- Checker: axe-core, vendored in `horcrux-api/horcrux-ui/e2e/a11y/vendor/` (version and checksum in `VERSION`, licence
  alongside). Nothing is fetched at test time. Update by replacing `axe.min.js` and `VERSION` together.
- Config: `e2e/a11y/a11y.json` holds the page list, the rule tags (WCAG 2.0/2.1/2.2 A and AA plus axe best practices),
  the impact threshold (`fail_impacts`: serious and critical fail the job; `list_impacts`: moderate and minor are
  printed) and the viewports and colour schemes.
- Allowlist: `e2e/a11y/a11y-allowlist.json`, only for true false positives. Each entry names the rule, the page, a
  selector fragment and the reason. It is empty today.
- axe "incomplete" results (cannot be decided automatically) are summarised in the output; `A11Y_VERBOSE=1` prints the
  nodes. Today that is only the phone menu toggle button, whose glyph has no text to measure.

Run locally: start the server as the E2E job does, then
`BASE_URL=https://localhost:8006 python3 horcrux-api/horcrux-ui/e2e/a11y_e2e.py`
(`A11Y_PAGES=login,vms`, `A11Y_WIDTHS=phone`, `A11Y_SCHEMES=dark`, `A11Y_REPORT=out.json` narrow or record a run).

Rules for UI code: every form control has a label (`<label for>` or `aria-label`), the page language is set, headings
step down one level at a time (a card title that is not a section heading uses `aria-level`), every `<nav>` has an
`aria-label`, an icon-only or empty header cell carries `.sr-only` text.
