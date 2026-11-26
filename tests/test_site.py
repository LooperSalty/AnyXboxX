"""Site GitHub Pages (site/index.html) : état publié, publié tel quel par .github/workflows/pages.yml."""
from pathlib import Path

RACINE = Path(__file__).resolve().parents[1]
SITE = RACINE / "site" / "index.html"


def test_site_states_the_game_runs():
    html = SITE.read_text(encoding="utf-8")
    assert 'id="verdict">It runs.</div>' in html
    assert "Not yet." not in html and "does not run yet" not in html


def test_site_links_to_the_launcher_guide_and_the_measured_figures():
    html = SITE.read_text(encoding="utf-8")
    assert "docs/launcher.md" in html
    assert "16–17" in html and "13.4 fps" in html


def test_site_has_no_leftover_estimator_script():
    assert not (RACINE / "site" / "estimate.js").exists()
    assert 'src="app.js"' not in SITE.read_text(encoding="utf-8")
