#!/usr/bin/env python3
"""
Recovery script: Reconstruct lost files from Claude Code's file-history
snapshots and Write tool calls in JSONL conversation logs.

Sources (in priority order):
1. file-history/ - Full file snapshots keyed by sha256(path)[:16], versioned
2. Write tool calls in JSONL logs - Full file content from Write operations
3. Read tool results in JSONL logs - File content with line numbers (parsed)

For each file we find the latest, largest version across all sources.
Then we apply any Edit operations that came after the last Write.
"""

import json
import os
import sys
import glob
import hashlib
from dataclasses import dataclass, field
from pathlib import Path


FILE_HISTORY_DIR = Path.home() / ".claude" / "file-history"
LOGS_DIR = Path.home() / ".claude" / "projects" / "-home-rich-src-HARES"
REPO_ROOT = Path.home() / "src" / "HARES"
RECOVERY_DIR = REPO_ROOT / ".recovery"
WORKTREE_PREFIX = str(REPO_ROOT / ".claude" / "worktrees")


def path_to_hash(file_path: str) -> str:
    """Convert a file path to the file-history hash key."""
    return hashlib.sha256(file_path.encode()).hexdigest()[:16]


@dataclass
class FileSnapshot:
    """A full file snapshot from any source."""
    file_path: str
    content: str
    source: str  # "file-history", "write-tool", "read-tool"
    timestamp: float  # mtime or ordering proxy
    version: int = 0  # for file-history versions


@dataclass
class EditOp:
    """An Edit operation from JSONL logs."""
    file_path: str
    old_string: str
    new_string: str
    replace_all: bool
    timestamp: float
    line_number: int


def scan_file_history() -> dict[str, list[FileSnapshot]]:
    """Scan file-history/ for all file snapshots.

    file-history/<session-id>/<sha256(path)[:16]>@v<N>
    We need to try all known HARES file paths against each hash.
    """
    print("Phase 1: Scanning file-history snapshots...")

    # First, collect all hashes that exist in file-history
    hash_to_files: dict[str, list[tuple[str, str, int]]] = {}  # hash -> [(session_dir, full_path, version)]

    for session_dir in FILE_HISTORY_DIR.iterdir():
        if not session_dir.is_dir():
            continue
        for entry in session_dir.iterdir():
            name = entry.name
            if "@v" not in name:
                continue
            hash_part, ver_part = name.rsplit("@v", 1)
            try:
                version = int(ver_part)
            except ValueError:
                continue
            hash_to_files.setdefault(hash_part, []).append(
                (str(session_dir), str(entry), version)
            )

    print(f"  Found {len(hash_to_files)} unique file hashes across file-history")

    # Now we need to figure out which file paths map to these hashes.
    # Strategy: compute hashes for all known paths (from Write calls, current repo, git history)
    known_paths = set()

    # Get all files currently in repo
    for root, _, files in os.walk(str(REPO_ROOT)):
        if ".git" in root or "target" in root or ".claude" in root or "vendor" in root:
            continue
        for f in files:
            known_paths.add(os.path.join(root, f))

    # Get all file paths from Write calls in JSONL logs (quick scan)
    for jf in glob.glob(str(LOGS_DIR / "**" / "*.jsonl"), recursive=True):
        try:
            with open(jf) as f:
                for line in f:
                    try:
                        d = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    if d.get("type") != "assistant":
                        continue
                    msg = d.get("message", {})
                    content = msg.get("content", [])
                    if not isinstance(content, list):
                        continue
                    for block in content:
                        if not isinstance(block, dict) or block.get("type") != "tool_use":
                            continue
                        inp = block.get("input", {})
                        if not isinstance(inp, dict):
                            continue
                        fp = inp.get("file_path", "")
                        if fp and fp.startswith(str(REPO_ROOT)):
                            known_paths.add(fp)
        except Exception:
            pass

    # Also get paths from Read calls
    for jf in glob.glob(str(LOGS_DIR / "**" / "*.jsonl"), recursive=True):
        try:
            with open(jf) as f:
                for line in f:
                    try:
                        d = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    if d.get("type") != "assistant":
                        continue
                    msg = d.get("message", {})
                    content = msg.get("content", [])
                    if not isinstance(content, list):
                        continue
                    for block in content:
                        if not isinstance(block, dict) or block.get("type") != "tool_use":
                            continue
                        if block.get("name") == "Read":
                            inp = block.get("input", {})
                            fp = inp.get("file_path", "")
                            if fp and fp.startswith(str(REPO_ROOT)):
                                known_paths.add(fp)
        except Exception:
            pass

    print(f"  Found {len(known_paths)} known file paths to check against hashes")

    # Build hash -> path mapping
    hash_to_path: dict[str, str] = {}
    for path in known_paths:
        h = path_to_hash(path)
        if h in hash_to_files:
            hash_to_path[h] = path

    print(f"  Matched {len(hash_to_path)} hashes to file paths")

    # Now read the actual file content
    snapshots: dict[str, list[FileSnapshot]] = {}
    for h, path in hash_to_path.items():
        if WORKTREE_PREFIX in path:
            continue
        for session_dir, full_path, version in hash_to_files[h]:
            try:
                content = Path(full_path).read_text(errors="replace")
                mtime = os.path.getmtime(full_path)
                snapshots.setdefault(path, []).append(FileSnapshot(
                    file_path=path,
                    content=content,
                    source="file-history",
                    timestamp=mtime,
                    version=version,
                ))
            except Exception as e:
                print(f"  WARNING: Could not read {full_path}: {e}", file=sys.stderr)

    print(f"  Loaded {sum(len(v) for v in snapshots.values())} snapshots for {len(snapshots)} files")
    return snapshots


def scan_jsonl_writes() -> tuple[dict[str, list[FileSnapshot]], dict[str, list[EditOp]]]:
    """Scan JSONL logs for Write and Edit tool calls."""
    print("\nPhase 2: Scanning JSONL logs for Write/Edit tool calls...")

    writes: dict[str, list[FileSnapshot]] = {}
    edits: dict[str, list[EditOp]] = {}

    jsonl_files = sorted(glob.glob(str(LOGS_DIR / "**" / "*.jsonl"), recursive=True))
    print(f"  Scanning {len(jsonl_files)} JSONL files...")

    for jf in jsonl_files:
        mtime = os.path.getmtime(jf)
        try:
            with open(jf) as f:
                for line_num, line in enumerate(f):
                    try:
                        d = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    if d.get("type") != "assistant":
                        continue
                    msg = d.get("message", {})
                    content = msg.get("content", [])
                    if not isinstance(content, list):
                        continue
                    for block in content:
                        if not isinstance(block, dict) or block.get("type") != "tool_use":
                            continue
                        name = block.get("name", "")
                        inp = block.get("input", {})
                        if not isinstance(inp, dict):
                            continue
                        fp = inp.get("file_path", "")
                        if not fp or not fp.startswith(str(REPO_ROOT)):
                            continue
                        if WORKTREE_PREFIX in fp:
                            continue

                        if name == "Write":
                            file_content = inp.get("content", "")
                            if file_content:
                                writes.setdefault(fp, []).append(FileSnapshot(
                                    file_path=fp,
                                    content=file_content,
                                    source=f"write:{os.path.basename(jf)}",
                                    timestamp=mtime + line_num * 0.0001,  # sub-second ordering
                                ))
                        elif name == "Edit":
                            old_str = inp.get("old_string", "")
                            new_str = inp.get("new_string", "")
                            replace_all = inp.get("replace_all", False)
                            if old_str or new_str:
                                edits.setdefault(fp, []).append(EditOp(
                                    file_path=fp,
                                    old_string=old_str,
                                    new_string=new_str,
                                    replace_all=replace_all,
                                    timestamp=mtime + line_num * 0.0001,
                                    line_number=line_num,
                                ))
        except Exception:
            pass

    total_writes = sum(len(v) for v in writes.values())
    total_edits = sum(len(v) for v in edits.values())
    print(f"  Found {total_writes} Write calls for {len(writes)} files")
    print(f"  Found {total_edits} Edit calls for {len(edits)} files")
    return writes, edits


def scan_jsonl_reads() -> dict[str, list[FileSnapshot]]:
    """Scan JSONL logs for Read tool calls and their results.

    Read calls appear in assistant messages; results appear in the following
    user message as tool_result blocks with the file content (prefixed with line numbers).
    """
    print("\nPhase 3: Scanning JSONL logs for Read tool results...")

    reads: dict[str, list[FileSnapshot]] = {}

    jsonl_files = sorted(glob.glob(str(LOGS_DIR / "**" / "*.jsonl"), recursive=True))

    for jf in jsonl_files:
        mtime = os.path.getmtime(jf)
        try:
            lines = open(jf).readlines()
        except Exception:
            continue

        # Track pending Read calls: tool_use_id -> (file_path, offset, limit)
        pending_reads: dict[str, tuple[str, int, int]] = {}

        for line_num, line in enumerate(lines):
            try:
                d = json.loads(line)
            except json.JSONDecodeError:
                continue

            if d.get("type") == "assistant":
                msg = d.get("message", {})
                content = msg.get("content", [])
                if not isinstance(content, list):
                    continue
                for block in content:
                    if not isinstance(block, dict) or block.get("type") != "tool_use":
                        continue
                    if block.get("name") != "Read":
                        continue
                    inp = block.get("input", {})
                    fp = inp.get("file_path", "")
                    if not fp or not fp.startswith(str(REPO_ROOT)):
                        continue
                    if WORKTREE_PREFIX in fp:
                        continue
                    offset = inp.get("offset", 0)
                    limit = inp.get("limit", 99999)
                    tool_id = block.get("id", "")
                    if tool_id:
                        pending_reads[tool_id] = (fp, offset, limit)

            elif d.get("type") == "user" and pending_reads:
                msg = d.get("message", {})
                content = msg.get("content", [])
                if not isinstance(content, list):
                    continue
                for block in content:
                    if not isinstance(block, dict) or block.get("type") != "tool_result":
                        continue
                    tool_id = block.get("tool_use_id", "")
                    if tool_id not in pending_reads:
                        continue
                    fp, offset, limit = pending_reads.pop(tool_id)

                    # Extract content from result
                    result_content = block.get("content", "")
                    text = ""
                    if isinstance(result_content, str):
                        text = result_content
                    elif isinstance(result_content, list):
                        for item in result_content:
                            if isinstance(item, dict) and "text" in item:
                                text = item["text"]
                                break

                    if not text or len(text) < 50:
                        continue

                    # Parse line-numbered content: "   123→content here"
                    # Only process full reads (offset=0, large limit)
                    if isinstance(offset, str) or offset > 0:
                        continue  # Skip partial reads

                    parsed_lines = []
                    for tl in text.split("\n"):
                        # Format: spaces + number + arrow/tab + content
                        if "→" in tl:
                            _, _, after = tl.partition("→")
                            parsed_lines.append(after)
                        elif "\t" in tl and tl.strip()[:1].isdigit():
                            _, _, after = tl.partition("\t")
                            parsed_lines.append(after)

                    if len(parsed_lines) > 10:  # Only if we got meaningful content
                        file_content = "\n".join(parsed_lines)
                        reads.setdefault(fp, []).append(FileSnapshot(
                            file_path=fp,
                            content=file_content,
                            source=f"read:{os.path.basename(jf)}",
                            timestamp=mtime + line_num * 0.0001,
                        ))

    total_reads = sum(len(v) for v in reads.values())
    print(f"  Extracted {total_reads} Read snapshots for {len(reads)} files")
    return reads


def apply_edits(content: str, edits: list[EditOp]) -> str:
    """Apply a sequence of Edit operations to content."""
    for edit in edits:
        if edit.replace_all:
            content = content.replace(edit.old_string, edit.new_string)
        else:
            idx = content.find(edit.old_string)
            if idx != -1:
                content = content[:idx] + edit.new_string + content[idx + len(edit.old_string):]
    return content


def pick_best_version(
    snapshots: list[FileSnapshot],
    edits: list[EditOp],
) -> str:
    """Pick the best version of a file from all available snapshots + edits.

    Strategy: The destructive operation wrote small stubs, so prefer the LARGEST
    snapshot. Among snapshots of similar size (within 20%), prefer the latest.
    Then apply any edits that came after.
    """
    if not snapshots:
        return ""

    # Primary sort: prefer largest content (the stubs from the reset are tiny)
    # Secondary sort: among similarly-sized snapshots, prefer latest
    max_size = max(len(s.content) for s in snapshots)

    def score(s: FileSnapshot) -> tuple[int, float]:
        size = len(s.content)
        # Group: 0 if within 80% of max, 1 if smaller
        size_group = 0 if size >= max_size * 0.8 else 1
        return (size_group, -s.timestamp)  # lower is better

    snapshots.sort(key=score)
    best = snapshots[0]
    content = best.content

    # Apply edits that came after the best snapshot
    if edits:
        later_edits = [e for e in edits if e.timestamp > best.timestamp]
        later_edits.sort(key=lambda e: (e.timestamp, e.line_number))
        if later_edits:
            content = apply_edits(content, later_edits)

    return content


def main():
    print("=" * 70)
    print("HARES Recovery - Extracting files from Claude agent logs + file-history")
    print("=" * 70)

    # Phase 1: file-history snapshots
    fh_snapshots = scan_file_history()

    # Phase 2: Write/Edit from JSONL
    write_snapshots, all_edits = scan_jsonl_writes()

    # Phase 3: Read results from JSONL
    read_snapshots = scan_jsonl_reads()

    # Merge all snapshots
    print("\n" + "=" * 70)
    print("MERGING SOURCES")
    print("=" * 70)

    all_paths = set()
    all_paths.update(fh_snapshots.keys())
    all_paths.update(write_snapshots.keys())
    all_paths.update(read_snapshots.keys())

    # Filter to HARES repo paths only (not worktrees)
    hares_paths = {
        p for p in all_paths
        if p.startswith(str(REPO_ROOT))
        and WORKTREE_PREFIX not in p
        and "/.git/" not in p
        and "/target/" not in p
    }

    print(f"  Total unique file paths across all sources: {len(hares_paths)}")

    # For each file, pick the best version
    best_versions: dict[str, str] = {}

    for fp in sorted(hares_paths):
        all_snaps = []
        all_snaps.extend(fh_snapshots.get(fp, []))
        all_snaps.extend(write_snapshots.get(fp, []))
        all_snaps.extend(read_snapshots.get(fp, []))

        file_edits = all_edits.get(fp, [])
        content = pick_best_version(all_snaps, file_edits)

        if content and content.strip():
            best_versions[fp] = content

    print(f"  Best versions recovered for {len(best_versions)} files")

    # Compare against current disk state
    print("\n" + "=" * 70)
    print("RECOVERY ANALYSIS")
    print("=" * 70)

    missing_files: dict[str, str] = {}
    different_files: dict[str, str] = {}
    same_files: list[str] = []
    stub_files: dict[str, str] = {}

    for fp, content in sorted(best_versions.items()):
        if not content.strip():
            continue

        disk_path = Path(fp)
        if not disk_path.exists():
            missing_files[fp] = content
        else:
            try:
                disk_content = disk_path.read_text()
                if disk_content.strip() == content.strip():
                    same_files.append(fp)
                elif len(disk_content.strip()) < 100 and len(content.strip()) > 200:
                    stub_files[fp] = content
                elif disk_content != content:
                    different_files[fp] = content
            except Exception:
                missing_files[fp] = content

    # Edit-only files (no snapshot, just edits)
    edit_only = set(all_edits.keys()) - set(best_versions.keys())
    edit_only = {p for p in edit_only if WORKTREE_PREFIX not in p}

    print(f"\n  Files matching disk:     {len(same_files)}")
    print(f"  Files MISSING from disk: {len(missing_files)}")
    print(f"  Files DIFFERENT on disk: {len(different_files)}")
    print(f"  Stub files (tiny→real):  {len(stub_files)}")
    print(f"  Edit-only (no base):     {len(edit_only)}")

    if missing_files:
        print(f"\n--- MISSING FILES ({len(missing_files)}) ---")
        for fp in sorted(missing_files):
            rel = os.path.relpath(fp, str(REPO_ROOT))
            print(f"  + {rel} ({len(missing_files[fp]):,} bytes)")

    if stub_files:
        print(f"\n--- STUB FILES → REAL CONTENT ({len(stub_files)}) ---")
        for fp in sorted(stub_files):
            rel = os.path.relpath(fp, str(REPO_ROOT))
            disk_sz = Path(fp).stat().st_size if Path(fp).exists() else 0
            print(f"  ~ {rel} (disk: {disk_sz}B → logs: {len(stub_files[fp]):,}B)")

    if different_files:
        print(f"\n--- DIFFERENT FILES ({len(different_files)}) ---")
        for fp in sorted(different_files):
            rel = os.path.relpath(fp, str(REPO_ROOT))
            disk_sz = Path(fp).stat().st_size if Path(fp).exists() else 0
            log_sz = len(different_files[fp])
            direction = "LARGER in logs" if log_sz > disk_sz else "smaller in logs"
            print(f"  ? {rel} (disk: {disk_sz:,}B, logs: {log_sz:,}B) [{direction}]")

    # Decide what to recover
    to_recover: dict[str, str] = {}
    to_recover.update(missing_files)
    to_recover.update(stub_files)

    # For different files, recover if log version is larger
    # Use lower threshold for critical config files (Cargo.toml, pyproject.toml, lib.rs)
    for fp, content in different_files.items():
        disk_sz = Path(fp).stat().st_size if Path(fp).exists() else 0
        log_sz = len(content)
        rel = os.path.relpath(fp, str(REPO_ROOT))

        # Skip vendor files - those are reference, not ours
        if rel.startswith("vendors/"):
            continue

        # Critical config/mod files: recover if log is bigger at all
        is_critical = any(rel.endswith(ext) for ext in [
            "Cargo.toml", "pyproject.toml", "lib.rs", "mod.rs",
        ])
        if is_critical and log_sz > disk_sz:
            to_recover[fp] = content
        elif log_sz > disk_sz * 1.5 and log_sz > 200:
            to_recover[fp] = content

    # Stage recovery
    print(f"\n{'=' * 70}")
    print("STAGING RECOVERY")
    print(f"{'=' * 70}")

    if not to_recover:
        print("\nNothing to recover!")
        return

    print(f"\nWill recover {len(to_recover)} files")

    recovery_dir = RECOVERY_DIR
    recovery_dir.mkdir(parents=True, exist_ok=True)

    # Write manifest
    manifest = []
    for fp in sorted(to_recover):
        rel = os.path.relpath(fp, str(REPO_ROOT))
        manifest.append(f"{rel}\t{len(to_recover[fp]):,} bytes")
    (recovery_dir / "MANIFEST.txt").write_text("\n".join(manifest) + "\n")

    # Stage files
    staged = 0
    for fp, content in sorted(to_recover.items()):
        rel = os.path.relpath(fp, str(REPO_ROOT))
        staged_path = recovery_dir / "staged" / rel
        staged_path.parent.mkdir(parents=True, exist_ok=True)
        staged_path.write_text(content)
        staged += 1

    print(f"Staged {staged} files in {recovery_dir / 'staged'}")

    # Write apply script
    apply_script = recovery_dir / "apply_recovery.sh"
    lines = [
        "#!/bin/bash",
        "set -euo pipefail",
        f"REPO='{REPO_ROOT}'",
        f"STAGED='{recovery_dir / 'staged'}'",
        "",
        "echo 'Applying recovered files...'",
    ]
    for fp in sorted(to_recover):
        rel = os.path.relpath(fp, str(REPO_ROOT))
        lines.append(f'mkdir -p "$(dirname "$REPO/{rel}")"')
        lines.append(f'cp "$STAGED/{rel}" "$REPO/{rel}"')
    lines.append(f"echo 'Done. Applied {len(to_recover)} files.'")
    apply_script.write_text("\n".join(lines) + "\n")
    apply_script.chmod(0o755)

    # Summary
    total_bytes = sum(len(c) for c in to_recover.values())
    print(f"\n{'=' * 70}")
    print("SUMMARY")
    print(f"{'=' * 70}")
    print(f"  Files to recover:  {len(to_recover)}")
    print(f"  Total bytes:       {total_bytes:,}")
    print(f"  Staged at:         {recovery_dir / 'staged'}")
    print(f"  Apply with:        bash {apply_script}")
    print(f"  Review first:      ls {recovery_dir / 'staged'}")


if __name__ == "__main__":
    main()
