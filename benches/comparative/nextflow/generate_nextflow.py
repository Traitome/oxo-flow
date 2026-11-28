#!/usr/bin/env python3
"""Generate the Nextflow definition for an N-step serial chain.

DSL2 permits a given process to be called only once per workflow, so the
chain is expressed as N include aliases of the shared ``echo_step`` process
(``nextflow/echo_step.nf``), each called once in dependency order. Generating
exactly N aliases keeps the definition size and parse cost proportional to the
workload, instead of carrying a fixed pre-expanded alias block.

Usage:
    python3 benches/comparative/nextflow/generate_nextflow.py N OUTPUT.nf
"""

import sys


def generate_nextflow(rule_count: int) -> str:
    """Return a DSL2 workflow calling ``echo_step`` rule_count times in series."""
    if rule_count < 1:
        raise ValueError("rule_count must be >= 1")

    lines = [
        "nextflow.enable.dsl = 2",
        "",
        f"// Generated for a chain of {rule_count} steps; one alias + one call per step.",
    ]
    for i in range(rule_count):
        lines.append(f"include {{ echo_step as step_{i} }} from './echo_step'")

    lines.extend(["", "workflow {"])
    lines.append("    def seed = file('input.txt', checkIfExists: false)")
    lines.append("    if (!seed.exists()) { seed.write('') }")
    lines.append("    def ch = step_0(seed, 0)")
    for i in range(1, rule_count):
        lines.append(f"    ch = step_{i}(ch, {i})")
    lines.extend(["}", ""])
    return "\n".join(lines)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: generate_nextflow.py N OUTPUT.nf")
    count = int(sys.argv[1])
    with open(sys.argv[2], "w") as handle:
        handle.write(generate_nextflow(count))
