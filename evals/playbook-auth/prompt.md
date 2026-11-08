---
max_turns: 6
timeout_seconds: 180
runs: 1
allowed_tools: [mcp__plugin_quome_quome__playbook]
---

Use the quome `playbook` tool to fetch the playbook named `auth`, with `what` set to "orient me". Do not run any of its steps. Tell me the first step of the playbook you received, name every tool and file it mentions exactly as written, and name the playbook.
