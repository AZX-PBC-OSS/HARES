"""SHA-256 sidecar helpers for cache integrity verification.

Downloaded files store a ``.sha256`` sidecar alongside the cached file.
On subsequent cache hits the stored hash is compared with the current
file content to detect bit-rot or truncated writes.

The pattern follows ``ochre_next.adapters.sam_pv._canonical_hash`` which
also uses ``hashlib.sha256`` for content verification.
"""

from __future__ import annotations

import hashlib
import logging
from pathlib import Path

log = logging.getLogger(__name__)


def sha256_path(filepath: Path) -> Path:
    """Return the ``.sha256`` sidecar path for *filepath*."""
    return filepath.with_suffix(filepath.suffix + ".sha256")


def compute_sha256_hex(filepath: Path) -> str:
    """Compute SHA-256 hex digest of *filepath* content."""
    return hashlib.sha256(filepath.read_bytes()).hexdigest()


def write_sha256_sidecar(filepath: Path) -> None:
    """Compute SHA-256 of *filepath* and write to ``.sha256`` sidecar."""
    digest = compute_sha256_hex(filepath)
    sp = sha256_path(filepath)
    sp.write_text(digest + "\n")


def validate_cache_integrity(filepath: Path) -> bool:
    """Return ``True`` if the cached file at *filepath* is safe to use.

    If a ``.sha256`` sidecar exists, verifies the file content matches the
    stored hash.  If no sidecar exists, the file is treated as valid (no
    integrity check is possible — this is the first-use case).

    Logs ``WARNING`` and returns ``False`` on hash mismatch, ``INFO`` on
    successful verification, and ``DEBUG`` when no sidecar exists.
    """
    sp = sha256_path(filepath)
    if not sp.exists():
        log.debug("No SHA256 sidecar for %s, skipping integrity check", filepath.name)
        return True
    stored = sp.read_text().strip()
    actual = compute_sha256_hex(filepath)
    if stored != actual:
        log.warning(
            "SHA256 mismatch for cached %s: expected=%s actual=%s",
            filepath.name, stored, actual,
        )
        return False
    log.info("SHA256 verified for cached %s", filepath.name)
    return True


def remove_cache_with_sidecar(filepath: Path) -> None:
    """Delete *filepath* and its ``.sha256`` sidecar if they exist."""
    filepath.unlink(missing_ok=True)
    sha256_path(filepath).unlink(missing_ok=True)
