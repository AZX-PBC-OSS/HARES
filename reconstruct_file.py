#!/usr/bin/env python3
"""
Reconstruct a single file from Claude Code history.

Usage: python3 reconstruct_file.py <file_path> [--dry-run] [--show-sources]

Sources searched (in order of completeness):
1. file-history/ snapshots (full file, versioned)
2. Write tool calls in JSONL logs (full file content)
3. Read tool results in JSONL logs (full file content with line numbers)
4. Edit tool calls applied on top of the best base

Strategy: Find ALL versions, pick the largest as base, replay edits on top.
"""

import json
import glob
import os
import sys
import hashlib
from pathlib import Path
from dataclasses import dataclass

FILE_HISTORY_DIR = Path.home() / ".claude" / "file-history"
LOGS_DIR = Path.home() / ".claude" / "projects" / "-home-rich-src-HARES"
REPO_ROOT = Path.home() / "src" / "HARES"


@dataclass
class Snapshot:
    content: str
    source: str
    timestamp: float
    size: int


@dataclass
class Edit:
    old_string: str
    new_string: str
    replace_all: bool
    timestamp: float
    source: str


def path_to_hash(fp: str) -> str:
    return hashlib.sha256(fp.encode()).hexdigest()[:16]


def find_file_history_snapshots(file_path: str) -> list[Snapshot]:
    """Find all file-history snapshots for this path."""
    h = path_to_hash(file_path)
    snapshots = []
    for entry in FILE_HISTORY_DIR.rglob(f"{h}@v*"):
        try:
            content = entry.read_text(errors="replace")
            mtime = entry.stat().st_mtime
            session = entry.parent.name[:8]
            snapshots.append(Snapshot(
                content=content,
                source=f"file-history/{session}.../{entry.name}",
                timestamp=mtime,
                size=len(content),
            ))
        except Exception:
            pass
    return snapshots


def find_write_snapshots(file_path: str) -> list[Snapshot]:
    """Find all Write tool calls for this path in JSONL logs."""
    snapshots = []
    for jf in glob.glob(str(LOGS_DIR / "**" / "*.jsonl"), recursive=True):
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
                    content = d.get("message", {}).get("content", [])
                    if not isinstance(content, list):
                        continue
                    for block in content:
                        if not isinstance(block, dict):
                            continue
                        if block.get("type") != "tool_use" or block.get("name") != "Write":
                            continue
                        inp = block.get("input", {})
                        if inp.get("file_path") != file_path:
                            continue
                        file_content = inp.get("content", "")
                        if file_content:
                            session = os.path.basename(jf)[:8]
                            snapshots.append(Snapshot(
                                content=file_content,
                                source=f"write/{session}...:{line_num}",
                                timestamp=mtime + line_num * 0.0001,
                                size=len(file_content),
                            ))
        except Exception:
            pass
    return snapshots


def find_read_snapshots(file_path: str) -> list[Snapshot]:
    """Find Read tool results for this path (full reads only)."""
    snapshots = []
    for jf in glob.glob(str(LOGS_DIR / "**" / "*.jsonl"), recursive=True):
        mtime = os.path.getmtime(jf)
        try:
            lines_list = open(jf).readlines()
        except Exception:
            continue

        pending: dict[str, tuple[int, int]] = {}  # tool_id -> (offset, limit)

        for line_num, line in enumerate(lines_list):
            try:
                d = json.loads(line)
            except json.JSONDecodeError:
                continue

            if d.get("type") == "assistant":
                msg_content = d.get("message", {}).get("content", [])
                if not isinstance(msg_content, list):
                    continue
                for block in msg_content:
                    if not isinstance(block, dict):
                        continue
                    if block.get("type") != "tool_use" or block.get("name") != "Read":
                        continue
                    inp = block.get("input", {})
                    if inp.get("file_path") != file_path:
                        continue
                    offset = inp.get("offset", 0)
                    limit = inp.get("limit", 99999)
                    tool_id = block.get("id", "")
                    if tool_id:
                        # Only track full reads (no offset)
                        if isinstance(offset, int) and offset == 0:
                            pending[tool_id] = (offset, limit)

            elif d.get("type") == "user" and pending:
                msg_content = d.get("message", {}).get("content", [])
                if not isinstance(msg_content, list):
                    continue
                for block in msg_content:
                    if not isinstance(block, dict) or block.get("type") != "tool_result":
                        continue
                    tool_id = block.get("tool_use_id", "")
                    if tool_id not in pending:
                        continue
                    pending.pop(tool_id)

                    result_content = block.get("content", "")
                    text = ""
                    if isinstance(result_content, str):
                        text = result_content
                    elif isinstance(result_content, list):
                        for item in result_content:
                            if isinstance(item, dict) and "text" in item:
                                text = item["text"]
                                break

                    if len(text) < 50:
                        continue

                    # Parse line-numbered content
                    parsed_lines = []
                    for tl in text.split("\n"):
                        if "→" in tl:
                            _, _, after = tl.partition("→")
                            parsed_lines.append(after)
                        elif "\t" in tl and tl.strip() and tl.strip()[0].isdigit():
                            _, _, after = tl.partition("\t")
                            parsed_lines.append(after)

                    if len(parsed_lines) > 5:
                        file_content = "\n".join(parsed_lines)
                        session = os.path.basename(jf)[:8]
                        snapshots.append(Snapshot(
                            content=file_content,
                            source=f"read/{session}...:{line_num}",
                            timestamp=mtime + line_num * 0.0001,
                            size=len(file_content),
                        ))
    return snapshots


def find_edits(file_path: str) -> list[Edit]:
    """Find all Edit tool calls for this path."""
    edits = []
    for jf in glob.glob(str(LOGS_DIR / "**" / "*.jsonl"), recursive=True):
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
                    content = d.get("message", {}).get("content", [])
                    if not isinstance(content, list):
                        continue
                    for block in content:
                        if not isinstance(block, dict):
                            continue
                        if block.get("type") != "tool_use" or block.get("name") != "Edit":
                            continue
                        inp = block.get("input", {})
                        if inp.get("file_path") != file_path:
                            continue
                        old = inp.get("old_string", "")
                        new = inp.get("new_string", "")
                        if old or new:
                            session = os.path.basename(jf)[:8]
                            edits.append(Edit(
                                old_string=old,
                                new_string=new,
                                replace_all=inp.get("replace_all", False),
                                timestamp=mtime + line_num * 0.0001,
                                source=f"edit/{session}...:{line_num}",
                            ))
        except Exception:
            pass
    return edits


def reconstruct(file_path: str, show_sources: bool = False) -> tuple[str, list[str]]:
    """Reconstruct a file from all available sources."""
    log = []

    # Gather all sources
    fh = find_file_history_snapshots(file_path)
    writes = find_write_snapshots(file_path)
    reads = find_read_snapshots(file_path)
    edits = find_edits(file_path)

    all_snapshots = fh + writes + reads
    log.append(f"Sources: {len(fh)} file-history, {len(writes)} writes, {len(reads)} reads, {len(edits)} edits")

    if show_sources:
        for s in sorted(all_snapshots, key=lambda s: s.timestamp):
            log.append(f"  {s.source}: {s.size:,}B")

    if not all_snapshots:
        log.append("NO SNAPSHOTS FOUND")
        return "", log

    # Pick the LARGEST snapshot as base
    largest = max(all_snapshots, key=lambda s: s.size)
    log.append(f"Base: {largest.source} ({largest.size:,}B)")
    content = largest.content

    # Apply edits that came AFTER the base
    edits.sort(key=lambda e: e.timestamp)
    applied = 0
    failed = 0
    for edit in edits:
        if edit.timestamp <= largest.timestamp:
            continue
        if edit.old_string in content:
            if edit.replace_all:
                content = content.replace(edit.old_string, edit.new_string)
            else:
                idx = content.find(edit.old_string)
                content = content[:idx] + edit.new_string + content[idx + len(edit.old_string):]
            applied += 1
        else:
            failed += 1
            if show_sources:
                log.append(f"  FAILED: {edit.source} old={len(edit.old_string)}B")

    if applied or failed:
        log.append(f"Edits: {applied} applied, {failed} failed")

    log.append(f"Final: {len(content):,}B, {content.count(chr(10))+1} lines")
    return content, log


def main():
    import argparse
    parser = argparse.ArgumentParser(description="Reconstruct a file from Claude logs")
    parser.add_argument("file_path", help="Absolute path to the file")
    parser.add_argument("--dry-run", action="store_true", help="Don't write, just show what would happen")
    parser.add_argument("--show-sources", action="store_true", help="Show all available sources")
    parser.add_argument("--diff", action="store_true", help="Show diff with current disk version")
    args = parser.parse_args()

    fp = args.file_path
    if not fp.startswith("/"):
        fp = str(REPO_ROOT / fp)

    content, log = reconstruct(fp, show_sources=args.show_sources)
    rel = os.path.relpath(fp, str(REPO_ROOT))

    print(f"=== {rel} ===")
    for l in log:
        print(f"  {l}")

    if not content:
        sys.exit(1)

    if args.diff and Path(fp).exists():
        disk = Path(fp).read_text()
        if disk == content:
            print("  MATCH: disk content is identical")
        else:
            print(f"  DIFF: disk={len(disk):,}B vs reconstructed={len(content):,}B")

    if not args.dry_run:
        Path(fp).parent.mkdir(parents=True, exist_ok=True)
        Path(fp).write_text(content)
        print(f"  Written to {fp}")
    else:
        print(f"  (dry-run, not written)")


if __name__ == "__main__":
    main()
