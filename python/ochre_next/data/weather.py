"""Weather data downloading and caching.

Provides EPW file fetching for ResStock buildings by FIPS code, using NREL's
BuildStock_TMY3_FIPS.zip dataset (https://data.nrel.gov/submissions/156).
"""

from __future__ import annotations

import os
import zipfile
from pathlib import Path


# NREL Data Catalog: TMY3 EPW files by county FIPS code.
# Updated Dec 2024; covers all US counties.
# Reference: https://data.nrel.gov/submissions/156
_TMY3_EPW_ZIP_URL = (
    "https://data.openei.org/files/156/BuildStock_TMY3_FIPS.zip"
)


def _default_weather_cache_dir() -> Path:
    """Return the default cache directory for weather files."""
    xdg = os.environ.get("XDG_CACHE_HOME")
    base = Path(xdg) if xdg else Path.home() / ".cache"
    return base / "ochre_next" / "weather"


def get_epw_for_fips(
    fips: str,
    cache_dir: Path | None = None,
) -> Path:
    """Return the path to a cached TMY3 EPW file for a county FIPS code.

    Downloads and extracts ``BuildStock_TMY3_FIPS.zip`` from NREL on first use,
    then serves individual EPW files from the local cache.

    Args:
        fips: County FIPS code (e.g. ``"G0800130"``).
        cache_dir: Override cache directory. Defaults to
            ``~/.cache/ochre_next/weather/``.

    Returns:
        Path to the cached ``.epw`` file.

    Raises:
        FileNotFoundError: If the FIPS code is not found in the ZIP archive.
        OSError: If the download fails (e.g. ``urllib.error.HTTPError``).
    """
    if cache_dir is None:
        cache_dir = _default_weather_cache_dir()

    epw_dir = cache_dir / "BuildStock_TMY3_FIPS"
    epw_path = epw_dir / f"{fips}.epw"

    if epw_path.exists() and epw_path.stat().st_size > 0:
        return epw_path

    # Check if we've already extracted the ZIP but this specific FIPS is missing
    marker = epw_dir / ".extracted"
    if marker.exists():
        raise FileNotFoundError(
            f"No EPW file for FIPS code {fips!r} in BuildStock_TMY3_FIPS. "
            f"Looked at: {epw_path}"
        )

    # Download and extract the full ZIP (760 MB, one-time operation)
    _ensure_tmy3_zip_extracted(epw_dir, cache_dir)

    if not epw_path.exists():
        raise FileNotFoundError(
            f"No EPW file for FIPS code {fips!r} in BuildStock_TMY3_FIPS. "
            f"Looked at: {epw_path}"
        )

    return epw_path


def _ensure_tmy3_zip_extracted(epw_dir: Path, cache_dir: Path) -> None:
    """Download and extract BuildStock_TMY3_FIPS.zip if not already done."""
    marker = epw_dir / ".extracted"
    if marker.exists():
        return

    zip_path = cache_dir / "BuildStock_TMY3_FIPS.zip"

    if not zip_path.exists() or zip_path.stat().st_size == 0:
        _download_large_file(_TMY3_EPW_ZIP_URL, zip_path)

    # Extract all EPW files to the cache directory.
    # Write each file to a .tmp path and atomically rename to avoid
    # partial reads when multiple processes extract concurrently.
    epw_dir.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(zip_path) as zf:
        for member in zf.namelist():
            if member.lower().endswith(".epw"):
                # Extract to flat directory (strip any subdirectory paths)
                filename = Path(member).name
                target = epw_dir / filename
                if not target.exists():
                    tmp_target = target.with_suffix(".epw.tmp")
                    with zf.open(member) as src, tmp_target.open("wb") as dst:
                        dst.write(src.read())
                    tmp_target.replace(target)

    # Write marker so we don't re-extract
    marker.write_text("ok")

    # Clean up the ZIP to save disk space (760 MB)
    zip_path.unlink(missing_ok=True)


def _download_large_file(url: str, dest: Path) -> None:
    """Download a large file with progress, using httpx, boto3, or urllib."""
    import urllib.request

    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".tmp")

    try:
        # Try httpx first (supports streaming + progress)
        try:
            import httpx  # type: ignore[import-not-found]

            with httpx.Client(follow_redirects=True, timeout=300.0) as client:
                with client.stream("GET", url) as resp:
                    resp.raise_for_status()
                    with tmp.open("wb") as fh:
                        for chunk in resp.iter_bytes(chunk_size=65536):
                            fh.write(chunk)
            tmp.replace(dest)
            return
        except ImportError:
            pass

        # Fallback to urllib
        with urllib.request.urlopen(url, timeout=300) as resp:  # noqa: S310
            with tmp.open("wb") as fh:
                while chunk := resp.read(65536):
                    fh.write(chunk)
        tmp.replace(dest)

    except Exception:
        tmp.unlink(missing_ok=True)
        raise
