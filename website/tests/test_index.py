# Verify the static site's visitor contracts without network access or browser dependencies.
import re
import unittest
from collections import Counter
from html.parser import HTMLParser
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.parse import unquote, urlsplit

PUBLIC = Path(__file__).resolve().parents[1] / "public"
PRODUCT_DESTINATIONS = {
    "keel": "https://github.com/OyadotAI/keel-releases/releases/latest",
    "oya-browser": "https://oyabrowser.com",
    "keel-reviewer": "https://github.com/OyadotAI/keel/tree/main/packaging/action/review",
}
MIN_ALT_LENGTH = 24
FIRST_HEADING = "h1"
NEXT_HEADING = "h2"


def local_asset(reference, base, root):
    """Resolve an asset within the deployable folder so hosting cannot expose repository files."""
    url = urlsplit(reference)
    if url.scheme or url.netloc or not url.path or url.path.startswith("/"):
        raise ValueError(f"Expected a relative local asset: {reference}")
    path = (base / unquote(url.path)).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError(f"Asset escapes the public folder: {reference}")
    if not path.is_file():
        raise ValueError(f"Missing asset: {reference}")
    return path


class Page(HTMLParser):
    """Collect public page contracts while keeping the checks independent of layout and prose."""

    def __init__(self, source):
        """Read the document once so all tests examine the same deployed artifact."""
        super().__init__()
        self.ids = []
        self.links = []
        self.assets = []
        self.images = []
        self.headings = []
        self.section_stack = []
        self.scripts = []
        self.languages = []
        self.feed(source)

    def handle_starttag(self, tag, attributes):
        """Record semantic relationships, including which product owns an outbound action."""
        attrs = dict(attributes)
        if "id" in attrs:
            self.ids.append(attrs["id"])
        if tag == "section":
            self.section_stack.append(attrs.get("id"))
        if tag == "a":
            section = self.section_stack[-1] if self.section_stack else None
            self.links.append((section, attrs.get("href", "")))
        self.collect_resources(tag, attrs)

    def collect_resources(self, tag, attrs):
        """Track resources and accessibility structure without requiring a rendering engine."""
        if tag in ("h1", "h2", "h3", "h4", "h5", "h6"):
            self.headings.append(tag)
        if tag == "html":
            self.languages.append(attrs.get("lang"))
        if tag == "script":
            self.scripts.append(attrs)
        if tag == "img":
            self.images.append(attrs)
            self.assets.append(attrs.get("src", ""))
        if tag == "link" and attrs.get("rel") != "canonical":
            self.assets.append(attrs.get("href", ""))

    def handle_endtag(self, tag):
        """Restore product ownership when a section ends, including nested sections."""
        if tag == "section" and self.section_stack:
            self.section_stack.pop()


class LandingPageTests(unittest.TestCase):
    """Pin product navigation, portability, and the readable no-script fallback."""

    @classmethod
    def setUpClass(cls):
        """Parse the actual shipping page instead of testing a second copy of its content."""
        cls.page = Page((PUBLIC / "index.html").read_text())

    def test_each_product_has_its_own_real_destination(self):
        for product, destination in PRODUCT_DESTINATIONS.items():
            with self.subTest(product=product):
                self.assertIn(product, self.page.ids)
                self.assertIn((product, destination), self.page.links)

    def test_fragment_destinations_are_present_and_unambiguous(self):
        duplicates = [name for name, count in Counter(self.page.ids).items() if count > 1]
        self.assertEqual([], duplicates)
        for _, reference in self.page.links:
            if reference.startswith("#") and reference != "#":
                self.assertIn(unquote(reference[1:]), self.page.ids, reference)

    def test_links_use_native_fragments_or_explicit_external_destinations(self):
        for _, reference in self.page.links:
            with self.subTest(reference=reference):
                self.assertTrue(reference)
                if not reference.startswith("#"):
                    self.assertIn(urlsplit(reference).scheme, ("https", "mailto"))

    def test_every_document_and_stylesheet_asset_ships_locally(self):
        self.assertTrue(self.page.assets)
        for reference in self.page.assets:
            local_asset(reference, PUBLIC, PUBLIC)
        for stylesheet in PUBLIC.glob("*.css"):
            for reference in re.findall(r"url\([\"']?([^\"')]+)", stylesheet.read_text()):
                local_asset(reference, stylesheet.parent, PUBLIC)
        self.assertTrue((PUBLIC / "assets" / "OFL.txt").is_file())

    def test_images_preserve_a_meaningful_text_fallback(self):
        self.assertTrue(self.page.images)
        for image in self.page.images:
            self.assertGreaterEqual(len(image.get("alt", "").strip()), MIN_ALT_LENGTH)
            self.assertTrue(image.get("width"))
            self.assertTrue(image.get("height"))

    def test_page_is_readable_and_navigable_without_javascript(self):
        self.assertEqual([], self.page.scripts)
        self.assertEqual(["en"], self.page.languages)
        self.assertEqual(FIRST_HEADING, self.page.headings[0])
        self.assertEqual(1, self.page.headings.count(FIRST_HEADING))
        self.assertTrue(all(tag == NEXT_HEADING for tag in self.page.headings[1:]))
        self.assertIn((None, "#main"), self.page.links)

    def test_local_assets_reject_missing_files_and_parent_traversal(self):
        with TemporaryDirectory() as scratch:
            root = Path(scratch) / "public"
            root.mkdir()
            (root.parent / "outside.txt").write_text("outside")
            for reference in ("missing.png", "../outside.txt", "%2e%2e/outside.txt", "/outside.txt"):
                with self.subTest(reference=reference), self.assertRaises(ValueError):
                    local_asset(reference, root, root)

    def test_local_assets_reject_remote_resources_and_escaping_symlinks(self):
        with TemporaryDirectory() as scratch:
            root = Path(scratch) / "public"
            root.mkdir()
            outside = root.parent / "outside.txt"
            outside.write_text("outside")
            (root / "escape.txt").symlink_to(outside)
            for reference in ("https://example.com/font.woff2", "//example.com/a.png", "escape.txt"):
                with self.subTest(reference=reference), self.assertRaises(ValueError):
                    local_asset(reference, root, root)
