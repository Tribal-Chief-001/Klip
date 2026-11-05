# Repository Guidelines & Operational Invariants

## 1. Resource Consciousness & Storage Guardrails
- **Pre-Execution & Routine Checks**:
  - Actively verify available disk storage (`df -h`) and memory (`free -m`) before and during heavy compilation, multi-crate testing, or subagent tasks.
  - If available disk space drops below 10 GB or memory usage approaches critical limits, alert the user and proactively clean ephemeral caches.
- **Proactive Ephemeral Pruning**:
  - Ephemeral build artifacts (such as intermediate objects in `target/`, `/tmp/`, task logs, and package caches) may be selectively pruned when needed to prevent disk exhaustion.
- **Strict Zero-Regression Invariant**:
  - **NEVER** delete, alter, or compromise user source code, git history, configurations, project assets, or documentation under resource pressure.
  - **NEVER** regress, weaken, or skip test suites, compilation rigor, or feature completeness.
  - Ephemeral cleanup is strictly restricted to regenerable build targets and temporary scratch caches.

## 2. Language & Authoring Invariants
- **Dialogue vs. Work Deliverables**:
  - Conversational chat with the user is conducted in natural Hinglish as preferred by the user.
  - **All formal work deliverables**—including source code, architectural documentation, git commits, PR descriptions, test cases, code comments, artifacts, and rule/configuration files—**MUST be authored strictly in standard, professional English**.
