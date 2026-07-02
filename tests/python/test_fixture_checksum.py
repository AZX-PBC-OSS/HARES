"""Tests for SHA256 manifest verification in download_resstock_fixtures.py (T-0355)."""

from __future__ import annotations

import hashlib
import importlib.util
from pathlib import Path
from unittest import mock

_SCRIPT = Path(__file__).resolve().parent.parent.parent / "scripts" / "download_resstock_fixtures.py"
_spec = importlib.util.spec_from_file_location("download_resstock_fixtures", str(_SCRIPT))
_script = importlib.util.module_from_spec(_spec)
with mock.patch("ochre_next.data.fetch_resstock_building"):
    _spec.loader.exec_module(_script)


# ---------------------------------------------------------------------------
# _compute_sha256
# ---------------------------------------------------------------------------


class TestComputeSha256:
    def test_known_hash(self, tmp_path: Path):
        f = tmp_path / "test.bin"
        f.write_bytes(b"hello, world\n")
        expected = hashlib.sha256(b"hello, world\n").hexdigest()
        assert _script._compute_sha256(f) == expected

    def test_empty_file(self, tmp_path: Path):
        f = tmp_path / "empty.bin"
        f.write_bytes(b"")
        expected = hashlib.sha256(b"").hexdigest()
        assert _script._compute_sha256(f) == expected

    def test_deterministic(self, tmp_path: Path):
        f = tmp_path / "data.bin"
        f.write_bytes(b"some data" * 100)
        h1 = _script._compute_sha256(f)
        h2 = _script._compute_sha256(f)
        assert h1 == h2


# ---------------------------------------------------------------------------
# _load_manifest / _write_manifest round-trip
# ---------------------------------------------------------------------------


class TestManifestRoundTrip:
    def test_round_trip(self, tmp_path: Path):
        entries = {
            "2024.2/bldg0000001/home.xml": "abc123",
            "2024.2/bldg0000001/in.schedules.csv": "def456",
        }
        manifest_path = tmp_path / "manifest.sha256"
        _script._write_manifest(manifest_path, entries)
        loaded = _script._load_manifest(manifest_path)
        assert loaded == entries

    def test_load_empty_file(self, tmp_path: Path):
        manifest_path = tmp_path / "manifest.sha256"
        manifest_path.write_text("")
        assert _script._load_manifest(manifest_path) == {}

    def test_load_nonexistent_file(self, tmp_path: Path):
        manifest_path = tmp_path / "nonexistent.sha256"
        assert _script._load_manifest(manifest_path) == {}

    def test_load_skips_comments_and_blanks(self, tmp_path: Path):
        manifest_path = tmp_path / "manifest.sha256"
        manifest_path.write_text(
            "# header comment\n"
            "abc123  path/to/file.xml\n"
            "\n"
            "  \n"
            "def456  other/file.csv\n"
        )
        loaded = _script._load_manifest(manifest_path)
        assert loaded == {"path/to/file.xml": "abc123", "other/file.csv": "def456"}

    def test_write_sorts_entries(self, tmp_path: Path):
        entries = {"z.txt": "aaa", "a.txt": "bbb", "m.txt": "ccc"}
        manifest_path = tmp_path / "manifest.sha256"
        _script._write_manifest(manifest_path, entries)
        lines = manifest_path.read_text().strip().split("\n")
        paths = [line.split("  ")[1] for line in lines]
        assert paths == sorted(paths)


# ---------------------------------------------------------------------------
# _verify_file_against_manifest
# ---------------------------------------------------------------------------


class TestVerifyFileAgainstManifest:
    def _file_with_content(self, tmp_path: Path, content: bytes, name: str = "test.xml") -> Path:
        f = tmp_path / name
        f.write_bytes(content)
        return f

    def _hash_of(self, content: bytes) -> str:
        return hashlib.sha256(content).hexdigest()

    def test_matching_hash_returns_true(self, tmp_path: Path):
        content = b"<xml>real content</xml>\n"
        f = self._file_with_content(tmp_path, content)
        expected_hash = self._hash_of(content)
        manifest = {"rel/path/file.xml": expected_hash}
        mismatches: list[tuple[str, str, str]] = []
        assert _script._verify_file_against_manifest(f, "rel/path/file.xml", manifest, mismatches) is True
        assert mismatches == []

    def test_mismatched_hash_returns_false_and_logs_error(self, tmp_path: Path):
        content = b"corrupted content\n"
        f = self._file_with_content(tmp_path, content)
        expected_hash = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        manifest = {"rel/path/file.xml": expected_hash}
        mismatches: list[tuple[str, str, str]] = []
        assert _script._verify_file_against_manifest(f, "rel/path/file.xml", manifest, mismatches) is False
        assert len(mismatches) == 1
        assert mismatches[0][0] == "rel/path/file.xml"
        assert mismatches[0][1] == expected_hash
        assert mismatches[0][2] == self._hash_of(content)

    def test_truncated_file_rejected(self, tmp_path: Path):
        """A file truncated to non-zero size but wrong content is rejected."""
        full_content = b"complete and correct fixture data\n" * 10
        truncated = full_content[:50]  # non-zero but wrong
        assert len(truncated) > 0
        assert truncated != full_content

        full_hash = self._hash_of(full_content)
        truncated_hash = self._hash_of(truncated)
        assert full_hash != truncated_hash  # different hashes

        f = self._file_with_content(tmp_path, truncated)
        manifest = {"rel/truncated.xml": full_hash}
        mismatches: list[tuple[str, str, str]] = []
        assert _script._verify_file_against_manifest(f, "rel/truncated.xml", manifest, mismatches) is False
        assert len(mismatches) == 1

    def test_new_entry_added_to_manifest(self, tmp_path: Path):
        content = b"fresh download content\n"
        f = self._file_with_content(tmp_path, content)
        manifest: dict[str, str] = {}
        mismatches: list[tuple[str, str, str]] = []
        assert _script._verify_file_against_manifest(f, "new/file.csv", manifest, mismatches) is True
        assert "new/file.csv" in manifest
        assert manifest["new/file.csv"] == self._hash_of(content)
        assert mismatches == []

    def test_new_entry_does_not_overwrite_existing(self, tmp_path: Path):
        content = b"existing content\n"
        f = self._file_with_content(tmp_path, content)
        existing_hash = self._hash_of(b"different content\n")
        manifest = {"path/file.xml": existing_hash}
        mismatches: list[tuple[str, str, str]] = []
        # File at "path/file.xml" matches? No, content differs.
        assert _script._verify_file_against_manifest(f, "path/file.xml", manifest, mismatches) is False
        # Manifest should NOT be overwritten — hash must still be the old one.
        assert manifest["path/file.xml"] == existing_hash

    def test_verification_loop_sets_changed_flag_on_new_entry(self, tmp_path: Path):
        """Simulate main()'s verification loop and confirm the manifest-changed
        flag flips when a new entry is added.  Regression for Finding 1:
        main() never set manifest_changed in the non-regenerate path, so
        newly computed hashes were silently discarded on shutdown."""
        existing_content = b"existing fixture\n"
        new_content = b"fresh download\n"
        f_existing = self._file_with_content(tmp_path, existing_content, "home.xml")
        f_new = self._file_with_content(tmp_path, new_content, "sched.csv")

        manifest: dict[str, str] = {
            "v1/bldg_1/home.xml": self._hash_of(existing_content),
        }
        mismatches: list[tuple[str, str, str]] = []
        changed = False

        files = [
            (f_existing, "v1/bldg_1/home.xml"),
            (f_new, "v1/bldg_1/in.schedules.csv"),
        ]
        for src, rel in files:
            is_new = rel not in manifest
            if not _script._verify_file_against_manifest(src, rel, manifest, mismatches):
                pass
            elif is_new:
                changed = True

        assert changed, "manifest_changed must be True after adding a new entry"
        assert "v1/bldg_1/home.xml" in manifest
        assert "v1/bldg_1/in.schedules.csv" in manifest
        assert manifest["v1/bldg_1/in.schedules.csv"] == self._hash_of(new_content)
        assert len(manifest) == 2
        assert mismatches == []

    def test_verification_loop_unchanged_when_all_entries_match(self, tmp_path: Path):
        """When every file already has a manifest entry and all hashes match,
        the manifest-changed flag must stay False — no write needed on shutdown."""
        content = b"fixture data\n"
        f1 = self._file_with_content(tmp_path, content, "home.xml")
        f2 = self._file_with_content(tmp_path, content, "sched.csv")

        manifest: dict[str, str] = {
            "v1/bldg_1/home.xml": self._hash_of(content),
            "v1/bldg_1/in.schedules.csv": self._hash_of(content),
        }
        mismatches: list[tuple[str, str, str]] = []
        changed = False

        for src, rel in [(f1, "v1/bldg_1/home.xml"), (f2, "v1/bldg_1/in.schedules.csv")]:
            is_new = rel not in manifest
            if not _script._verify_file_against_manifest(src, rel, manifest, mismatches):
                pass
            elif is_new:
                changed = True

        assert not changed, "manifest_changed must be False when no new entries added"
        assert mismatches == []


# ---------------------------------------------------------------------------
# _regenerate_manifest
# ---------------------------------------------------------------------------


class TestRegenerateManifest:
    def test_computes_hashes_for_all_files(self, tmp_path: Path):
        (tmp_path / "2024.2" / "bldg0000001").mkdir(parents=True)
        (tmp_path / "2024.2" / "weather").mkdir(parents=True)
        (tmp_path / "manifest.sha256").write_text("# existing\n")

        home = tmp_path / "2024.2" / "bldg0000001" / "home.xml"
        sched = tmp_path / "2024.2" / "bldg0000001" / "in.schedules.csv"
        weather = tmp_path / "2024.2" / "weather" / "G0800130.epw"

        home.write_bytes(b"home")
        sched.write_bytes(b"schedule")
        weather.write_bytes(b"weather")

        entries = _script._regenerate_manifest(tmp_path)
        assert len(entries) == 3
        # manifest.sha256 itself must be excluded
        assert "manifest.sha256" not in entries
        assert entries["2024.2/bldg0000001/home.xml"] == _script._compute_sha256(home)
        assert entries["2024.2/bldg0000001/in.schedules.csv"] == _script._compute_sha256(sched)
        assert entries["2024.2/weather/G0800130.epw"] == _script._compute_sha256(weather)

    def test_posix_path_separators(self, tmp_path: Path):
        (tmp_path / "v1" / "weather").mkdir(parents=True)
        f = tmp_path / "v1" / "weather" / "data.csv"
        f.write_bytes(b"data")
        entries = _script._regenerate_manifest(tmp_path)
        assert entries["v1/weather/data.csv"] == _script._compute_sha256(f)

    def test_empty_directory(self, tmp_path: Path):
        entries = _script._regenerate_manifest(tmp_path)
        assert entries == {}

    def test_excludes_unrelated_files(self, tmp_path: Path):
        """Files outside the script's output contract (.idf, .osm) are excluded.
        Regression for Finding 2 where rglob included nested 2025.1/2025.1/ artifacts."""
        (tmp_path / "v1" / "bldg_1").mkdir(parents=True)
        (tmp_path / "v1" / "weather").mkdir(parents=True)

        home = tmp_path / "v1" / "bldg_1" / "home.xml"
        sched = tmp_path / "v1" / "bldg_1" / "in.schedules.csv"
        idf = tmp_path / "v1" / "bldg_1" / "in.idf"
        osm = tmp_path / "v1" / "bldg_1" / "in.osm"
        weather = tmp_path / "v1" / "weather" / "data.csv"

        home.write_bytes(b"home")
        sched.write_bytes(b"sched")
        idf.write_bytes(b"idf")
        osm.write_bytes(b"osm")
        weather.write_bytes(b"weather")

        entries = _script._regenerate_manifest(tmp_path)

        assert "v1/bldg_1/home.xml" in entries
        assert "v1/bldg_1/in.schedules.csv" in entries
        assert "v1/weather/data.csv" in entries
        assert "v1/bldg_1/in.idf" not in entries
        assert "v1/bldg_1/in.osm" not in entries
        assert len(entries) == 3

    def test_excludes_nested_version_directory(self, tmp_path: Path):
        """Files in a nested {version}/{version}/ directory are excluded.
        Regression for Finding 2: rglob swept 16 files from a pre-existing
        2025.1/2025.1/ directory into the manifest as if they were
        script-managed fixtures."""
        (tmp_path / "v1" / "bldg_1").mkdir(parents=True)
        (tmp_path / "v1" / "weather").mkdir(parents=True)
        (tmp_path / "v1" / "v1" / "bldg_2").mkdir(parents=True)
        (tmp_path / "v1" / "v1" / "weather").mkdir(parents=True)

        for path, data in [
            (tmp_path / "v1" / "bldg_1" / "home.xml", b"home"),
            (tmp_path / "v1" / "bldg_1" / "in.schedules.csv", b"sched"),
            (tmp_path / "v1" / "weather" / "data.csv", b"weather"),
            (tmp_path / "v1" / "v1" / "bldg_2" / "home.xml", b"nested_home"),
            (tmp_path / "v1" / "v1" / "bldg_2" / "in.schedules.csv", b"nested_sched"),
            (tmp_path / "v1" / "v1" / "bldg_2" / "in.idf", b"nested_idf"),
            (tmp_path / "v1" / "v1" / "weather" / "data.csv", b"nested_weather"),
        ]:
            path.write_bytes(data)

        entries = _script._regenerate_manifest(tmp_path)

        assert "v1/bldg_1/home.xml" in entries
        assert "v1/bldg_1/in.schedules.csv" in entries
        assert "v1/weather/data.csv" in entries
        assert "v1/v1/bldg_2/home.xml" not in entries
        assert "v1/v1/bldg_2/in.schedules.csv" not in entries
        assert "v1/v1/bldg_2/in.idf" not in entries
        assert "v1/v1/weather/data.csv" not in entries
        assert len(entries) == 3
