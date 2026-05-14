#!/usr/bin/env python3
"""Summarize termlm-test JSON output into a tuning-focused Markdown report."""

from __future__ import annotations

import argparse
import json
import statistics
from pathlib import Path
from typing import Any


def load_json(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def pct(numerator: int, denominator: int) -> str:
    if denominator <= 0:
        return "n/a"
    return f"{(numerator / denominator) * 100:.1f}%"


def fmt_float(value: Any) -> str:
    if isinstance(value, (int, float)):
        return f"{value:.2f}"
    return "n/a"


def latency_summary(values: list[int]) -> str:
    if not values:
        return "n/a"
    values = sorted(values)
    p50 = statistics.median(values)
    p95_idx = min(len(values) - 1, max(0, round((len(values) - 1) * 0.95)))
    return f"p50={p50:.0f} ms, p95={values[p95_idx]} ms, max={values[-1]} ms"


def markdown_escape(value: Any) -> str:
    text = "" if value is None else str(value)
    return text.replace("|", "\\|").replace("\n", "<br>")


def collect_failures(results: list[tuple[Path, dict[str, Any]]]) -> list[tuple[Path, dict[str, Any]]]:
    failures: list[tuple[Path, dict[str, Any]]] = []
    for path, data in results:
        for test in data.get("tests", []):
            if not test.get("passed", False):
                failures.append((path, test))
    return failures


def collect_retrieval_misses(
    results: list[tuple[Path, dict[str, Any]]],
) -> list[tuple[Path, dict[str, Any]]]:
    misses: list[tuple[Path, dict[str, Any]]] = []
    for path, data in results:
        for test in data.get("tests", []):
            score = test.get("retrieval_score")
            if score and not score.get("hit", False):
                misses.append((path, test))
    return misses


def stage_latency_rows(results: list[tuple[Path, dict[str, Any]]]) -> list[tuple[str, str]]:
    values: dict[str, list[int]] = {}
    for _path, data in results:
        for test in data.get("tests", []):
            for key, value in test.get("stage_timings_ms", {}).items():
                if isinstance(value, int):
                    values.setdefault(key, []).append(value)
    return [(key, latency_summary(samples)) for key, samples in sorted(values.items())]


def write_report(inputs: list[Path], output: Path) -> None:
    loaded = [(path, load_json(path)) for path in inputs]
    lines: list[str] = []
    lines.append("# termlm Accuracy Report")
    lines.append("")
    lines.append("This report is generated from `termlm-test` JSON output and is meant to drive prompt, retrieval, planner, validation, and tool-orchestration tuning. Do not use it as a source for prompt-specific production shortcuts.")
    lines.append("")
    lines.append("## Inputs")
    lines.append("")
    lines.append("| File | Suite | Model | Duration | Passed | Failed | Retrieval top1 | Retrieval top-k |")
    lines.append("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |")
    for path, data in loaded:
        summary = data.get("summary", {})
        env = data.get("benchmark_environment", {})
        lines.append(
            "| "
            + " | ".join(
                [
                    markdown_escape(path),
                    markdown_escape(data.get("suite_version", "")),
                    markdown_escape(env.get("model", "")),
                    markdown_escape(f"{data.get('duration_secs', 0)}s"),
                    markdown_escape(summary.get("passed", 0)),
                    markdown_escape(summary.get("failed", 0)),
                    markdown_escape(fmt_float(summary.get("retrieval_hit_rate_top1"))),
                    markdown_escape(fmt_float(summary.get("retrieval_hit_rate_top5"))),
                ]
            )
            + " |"
        )
    lines.append("")

    lines.append("## Category Pass Rates")
    lines.append("")
    lines.append("| File | Category | Passed | Total | Rate |")
    lines.append("| --- | --- | ---: | ---: | ---: |")
    for path, data in loaded:
        by_category = data.get("summary", {}).get("by_category", {})
        for category, summary in sorted(by_category.items()):
            passed = int(summary.get("passed", 0))
            total = int(summary.get("total", 0))
            lines.append(
                f"| {markdown_escape(path.name)} | {markdown_escape(category)} | {passed} | {total} | {pct(passed, total)} |"
            )
    lines.append("")

    failures = collect_failures(loaded)
    lines.append("## Failures")
    lines.append("")
    if failures:
        lines.append("| File | Test | Category | Mode | Prompt | Proposed command | Exit | Error |")
        lines.append("| --- | --- | --- | --- | --- | --- | ---: | --- |")
        for path, test in failures:
            lines.append(
                "| "
                + " | ".join(
                    [
                        markdown_escape(path.name),
                        markdown_escape(test.get("id", "")),
                        markdown_escape(test.get("category", "")),
                        markdown_escape(test.get("mode", "")),
                        markdown_escape(test.get("prompt", "")),
                        markdown_escape(test.get("proposed_command", "")),
                        markdown_escape(test.get("exit_status", "")),
                        markdown_escape(test.get("error", "")),
                    ]
                )
                + " |"
            )
    else:
        lines.append("No failing tests.")
    lines.append("")

    misses = collect_retrieval_misses(loaded)
    lines.append("## Retrieval Misses")
    lines.append("")
    if misses:
        lines.append("| File | Test | Category | Prompt | Top-k | Best rank | Error |")
        lines.append("| --- | --- | --- | --- | ---: | ---: | --- |")
        for path, test in misses:
            score = test.get("retrieval_score") or {}
            lines.append(
                "| "
                + " | ".join(
                    [
                        markdown_escape(path.name),
                        markdown_escape(test.get("id", "")),
                        markdown_escape(test.get("category", "")),
                        markdown_escape(test.get("prompt", "")),
                        markdown_escape(score.get("top_k", "")),
                        markdown_escape(score.get("best_rank", "")),
                        markdown_escape(test.get("error", "")),
                    ]
                )
                + " |"
            )
    else:
        lines.append("No retrieval misses.")
    lines.append("")

    rows = stage_latency_rows(loaded)
    if rows:
        lines.append("## Stage Timings")
        lines.append("")
        lines.append("| Stage | Latency |")
        lines.append("| --- | --- |")
        for stage, summary in rows:
            lines.append(f"| {markdown_escape(stage)} | {markdown_escape(summary)} |")
        lines.append("")

    lines.append("## Tuning Packet")
    lines.append("")
    lines.append("Use this section as input to an LLM or manual tuning pass:")
    lines.append("")
    lines.append("- Prefer changes to retrieval ranking, document expansion/rationalization, planner prompts, validation feedback, safe informational tool-call orchestration, and fallback/clarification behavior.")
    lines.append("- Do not add production branches that match one test prompt and emit one canned command.")
    lines.append("- For command failures, inspect whether retrieval produced the right command docs, whether the planner over-constrained or under-specified the command, and whether validation feedback caused a useful repair loop.")
    lines.append("- For answer failures, tune the answer-vs-command decision and make blank/timeout outcomes explain themselves with a follow-up question.")
    lines.append("- For retrieval misses, tune command-document generation, BM25 text, embedding chunks, synonyms, and rank fusion before touching prompt behavior.")
    lines.append("")
    lines.append("```json")
    lines.append(
        json.dumps(
            {
                "failures": [
                    {
                        "file": path.name,
                        "id": test.get("id"),
                        "prompt": test.get("prompt"),
                        "category": test.get("category"),
                        "mode": test.get("mode"),
                        "proposed_command": test.get("proposed_command"),
                        "exit_status": test.get("exit_status"),
                        "error": test.get("error"),
                        "stage_timings_ms": test.get("stage_timings_ms", {}),
                    }
                    for path, test in failures[:50]
                ],
                "retrieval_misses": [
                    {
                        "file": path.name,
                        "id": test.get("id"),
                        "prompt": test.get("prompt"),
                        "category": test.get("category"),
                        "retrieval_score": test.get("retrieval_score"),
                        "error": test.get("error"),
                    }
                    for path, test in misses[:50]
                ],
            },
            indent=2,
            sort_keys=True,
        )
    )
    lines.append("```")
    lines.append("")

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(lines), encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", action="append", required=True, help="termlm-test JSON file")
    parser.add_argument("--out", required=True, help="Markdown report path")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    inputs = [Path(raw) for raw in args.input]
    missing = [str(path) for path in inputs if not path.exists()]
    if missing:
        raise SystemExit(f"missing result file(s): {', '.join(missing)}")
    write_report(inputs, Path(args.out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
