# Allternit Memory Drive plugin

Lets Claude Code, Codex or any agent with a shell use an Allternit Memory Drive:
load `MEMORY.md` at session start, read topic files, and save new memories with
a source and date. Install as a Claude Code plugin (this folder) or copy
`skills/memory-drive/SKILL.md` and `scripts/memory-drive.sh` into another
agent's skills folder.

- Setup and rules: `skills/memory-drive/SKILL.md`
- Local test (no server needed): `bash tests/test.sh`
- User guide: https://docs.allternit.com/guides/memory-drive

The memory format is the Agent Memory Repo SPEC.md, MIT licensed
(https://github.com/AgentMemoryRepo/agentmemoryrepo). See NOTICE.
