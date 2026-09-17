#!/usr/bin/env python3
"""Skyline: lightweight shared situational awareness for autonomous model workers.

Skyline persists mission context, worker-authored intent, evidence, peer messages,
and explicit shared-resource coordination. It is not a scheduler, allocator, role
manager, task queue, work timer, permission boundary, or execution harness.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import secrets
import shutil
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Iterator

try:
    import fcntl
except ImportError:  # pragma: no cover - exercised on Windows
    fcntl = None

try:
    import msvcrt
except ImportError:  # pragma: no cover - exercised on POSIX
    msvcrt = None

SCHEMA_VERSION = 7
DEFAULT_ARCHIVE_AFTER_SECONDS = 2 * 60 * 60
DEFAULT_STALE_AFTER_SECONDS = 30 * 60
DEFAULT_MAX_ACTIVE_AGENTS = 64  # legacy claim compatibility only
DEFAULT_BOOTSTRAP_BROADCAST_HISTORY_SECONDS = 6 * 60 * 60
DEFAULT_MAX_PEERS = 12
DEFAULT_MAX_MESSAGES = 12
DEFAULT_MAX_OPPORTUNITIES = 12
DEFAULT_RECENT_OUTCOMES = 8
DEFAULT_RECENT_TEST_RUNS = 6
DEFAULT_COORDINATION_RETENTION_SECONDS = 24 * 60 * 60
DEFAULT_PRIORITY_COORDINATION_RETENTION_SECONDS = 3 * 24 * 60 * 60
DEFAULT_USER_RELAYED_COORDINATION_RETENTION_SECONDS = 7 * 24 * 60 * 60
DEFAULT_MAINTENANCE_INTERVAL_SECONDS = 5 * 60
DEFAULT_TEST_RUN_STALE_SECONDS = 4 * 60 * 60
INACTIVE_AGENT_STATUSES = {"STOPPED", "DONE", "EXITED", "CLOSED", "IDLE", "COMPLETED", "FINISHED", "RETIRED", "STALE_LEGACY"}
CURRENT_OPPORTUNITY_STATES = ("pending", "active", "review")
ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$")
DIVISION_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")


class SkylineError(RuntimeError):
    pass


def utcnow() -> datetime:
    return datetime.now(timezone.utc)


def iso_now() -> str:
    return utcnow().isoformat()


def parse_time(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def slugify(value: str, limit: int = 48) -> str:
    value = re.sub(r"[^A-Za-z0-9]+", "-", value.strip().lower()).strip("-")
    return (value or "general")[:limit]


def validate_id(value: str, label: str = "id") -> str:
    value = value.strip()
    if not ID_RE.fullmatch(value):
        raise SkylineError(f"invalid {label}: {value!r}")
    return value


def validate_division(value: str) -> str:
    value = slugify(value, 64)
    if not DIVISION_RE.fullmatch(value):
        raise SkylineError(f"invalid division: {value!r}")
    return value


def positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected a positive integer") from exc
    if parsed < 1:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return parsed


def skyline_dir(root: str | Path) -> Path:
    path = Path(root).expanduser().resolve()
    return path if path.name == "skyline" else path / "skyline"


def json_read(path: Path, default: Any = None) -> Any:
    if not path.exists():
        return default
    return json.loads(path.read_text(encoding="utf-8"))


def touch_opportunity_revision_for_path(path: Path) -> None:
    if path.suffix != ".json" or path.parent.name not in CURRENT_OPPORTUNITY_STATES or path.parent.parent.name != "jobs":
        return
    sky = path.parent.parent.parent
    marker = sky / ".opportunity-revision"
    tmp = marker.with_name(f".{marker.name}.{os.getpid()}.{secrets.token_hex(3)}.tmp")
    tmp.write_text(secrets.token_hex(12), encoding="ascii")
    os.replace(tmp, marker)


def json_write(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.{os.getpid()}.{secrets.token_hex(4)}.tmp")
    tmp.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(tmp, path)
    touch_opportunity_revision_for_path(path)


def emit(value: Any) -> None:
    print(json.dumps(value, indent=2, ensure_ascii=False))


def config_path(sky: Path) -> Path:
    return sky / "CONFIG.json"


def load_config(sky: Path) -> dict[str, Any]:
    cfg = json_read(config_path(sky), {}) or {}
    cfg.setdefault("schema_version", SCHEMA_VERSION)
    cfg.setdefault("archive_after_seconds", DEFAULT_ARCHIVE_AFTER_SECONDS)
    cfg.setdefault("stale_after_seconds", DEFAULT_STALE_AFTER_SECONDS)
    cfg.setdefault("coordination_mode", "autonomous")
    cfg.setdefault("claim_mode", "legacy_advisory")
    cfg.setdefault("snapshot_mode", "live_delta")
    cfg.setdefault("max_active_agents", DEFAULT_MAX_ACTIVE_AGENTS)
    cfg.setdefault("bootstrap_broadcast_history_seconds", DEFAULT_BOOTSTRAP_BROADCAST_HISTORY_SECONDS)
    cfg.setdefault("max_peer_intents", DEFAULT_MAX_PEERS)
    cfg.setdefault("max_messages", DEFAULT_MAX_MESSAGES)
    cfg.setdefault("max_opportunities", DEFAULT_MAX_OPPORTUNITIES)
    cfg.setdefault("recent_outcomes", DEFAULT_RECENT_OUTCOMES)
    cfg.setdefault("recent_test_runs", DEFAULT_RECENT_TEST_RUNS)
    cfg.setdefault("coordination_retention_seconds", DEFAULT_COORDINATION_RETENTION_SECONDS)
    cfg.setdefault("priority_coordination_retention_seconds", DEFAULT_PRIORITY_COORDINATION_RETENTION_SECONDS)
    cfg.setdefault("user_relayed_coordination_retention_seconds", DEFAULT_USER_RELAYED_COORDINATION_RETENTION_SECONDS)
    cfg.setdefault("maintenance_interval_seconds", DEFAULT_MAINTENANCE_INTERVAL_SECONDS)
    cfg.setdefault("test_run_stale_after_seconds", DEFAULT_TEST_RUN_STALE_SECONDS)
    try:
        cfg["max_active_agents"] = max(1, min(128, int(cfg["max_active_agents"])))
    except (TypeError, ValueError):
        cfg["max_active_agents"] = DEFAULT_MAX_ACTIVE_AGENTS
    for key, default, minimum in (
        ("coordination_retention_seconds", DEFAULT_COORDINATION_RETENTION_SECONDS, 60),
        ("priority_coordination_retention_seconds", DEFAULT_PRIORITY_COORDINATION_RETENTION_SECONDS, 60),
        ("user_relayed_coordination_retention_seconds", DEFAULT_USER_RELAYED_COORDINATION_RETENTION_SECONDS, 60),
        ("maintenance_interval_seconds", DEFAULT_MAINTENANCE_INTERVAL_SECONDS, 30),
        ("test_run_stale_after_seconds", DEFAULT_TEST_RUN_STALE_SECONDS, 300),
    ):
        try:
            cfg[key] = max(minimum, int(cfg[key]))
        except (TypeError, ValueError):
            cfg[key] = default
    return cfg


def require_initialized(sky: Path) -> None:
    if not config_path(sky).exists():
        raise SkylineError(f"Skyline is not initialized at {sky}; run `skyline.py init --root ...`")


@contextlib.contextmanager
def locked(sky: Path) -> Iterator[None]:
    lock = sky / ".skyline.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open("a+") as handle:
        if fcntl is not None:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        elif msvcrt is not None:
            # msvcrt.locking locks bytes from the current file position and
            # requires the byte to exist before it can acquire the lock.
            handle.seek(0, os.SEEK_END)
            if handle.tell() == 0:
                handle.write("0")
                handle.flush()
            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_LOCK, 1)
            try:
                yield
            finally:
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
        else:  # pragma: no cover - Python platforms without file locking
            yield


def find_skyline(start: str | Path) -> Path | None:
    path = Path(start).expanduser().resolve()
    if path.is_file():
        path = path.parent
    for candidate in (path, *path.parents):
        if candidate.name == "skyline" and config_path(candidate).exists():
            return candidate
        nested = candidate / "skyline"
        if config_path(nested).exists():
            return nested
    return None


def deploy_runtime(sky: Path) -> Path:
    source = Path(__file__).resolve()
    destination = sky / "skyline.py"
    same_file = destination.exists() and source == destination.resolve()
    if not same_file:
        shutil.copy2(source, destination)
        destination.chmod(0o755)
    return destination


def claim_path(sky: Path, job_id: str) -> Path:
    return sky / "claims" / f"{validate_id(job_id, 'job id')}.json"


def lease_expiry(sky: Path) -> str:
    cfg = load_config(sky)
    return (utcnow() + timedelta(seconds=int(cfg["stale_after_seconds"]))).isoformat()


def verify_claim_locked(sky: Path, agent_id: str, claim_id: str) -> tuple[Path, dict[str, Any], Path, dict[str, Any], dict[str, Any]]:
    agent_path, agent = load_agent(sky, agent_id)
    current = current_job_by_agent(sky, agent_id)
    if not current:
        raise SkylineError(f"agent {agent_id} no longer owns an active job")
    _, job_path, job = current
    if agent.get("current_job") != job.get("id"):
        raise SkylineError(f"agent {agent_id} state/job mismatch")
    if agent.get("current_claim_id") != claim_id or job.get("claim_id") != claim_id:
        raise SkylineError(f"legacy advisory claim lost for agent {agent_id}; reassess overlapping mutation before relying on it; unrelated work is unaffected")
    record = json_read(claim_path(sky, job["id"]), None)
    if not record:
        raise SkylineError(f"legacy advisory claim record missing for job {job['id']}; reassess only work that depends on this reservation")
    if record.get("agent_id") != agent_id or record.get("claim_id") != claim_id:
        raise SkylineError(f"legacy advisory claim superseded for job {job['id']}; treat this as an overlap signal, not a global stop condition")
    if int(record.get("generation", -1)) != int(job.get("claim_generation", -2)):
        raise SkylineError(f"legacy advisory claim generation mismatch for job {job['id']}; reassess overlapping writes before relying on it")
    expires = parse_time(record.get("lease_expires_at"))
    if expires is None or utcnow() > expires:
        raise SkylineError(f"legacy advisory claim lease expired for job {job['id']}; renew only if that reservation is still useful")
    return agent_path, agent, job_path, job, record


def renew_claim_locked(sky: Path, agent_path: Path, agent: dict[str, Any], job_path: Path, job: dict[str, Any], record: dict[str, Any]) -> None:
    expires_at = lease_expiry(sky)
    now = iso_now()
    record["lease_expires_at"] = expires_at
    record["heartbeat_at"] = now
    job["lease_expires_at"] = expires_at
    job["updated_at"] = now
    agent["last_heartbeat"] = now
    json_write(claim_path(sky, job["id"]), record)
    json_write(job_path, job)
    save_agent(agent_path, agent)


def release_claim_locked(sky: Path, job: dict[str, Any]) -> None:
    path = claim_path(sky, job["id"])
    path.unlink(missing_ok=True)
    job["claim_id"] = None
    job["lease_expires_at"] = None


def ensure_layout(sky: Path) -> None:
    for rel in (
        "divisions",
        "jobs/pending",
        "jobs/active",
        "jobs/review",
        "jobs/done",
        "jobs/archive",
        "claims",
        "intents",
        "outcomes",
        "agents",
        "archive/agents",
        "validations",
        "messages/all",
        "messages/jobs",
        "shared",
        "decisions",
        "reactions",
        "whiteboard/cards",
        "test-runs/active",
        "test-runs/history",
        "archive/coordination",
        "tmp",
    ):
        (sky / rel).mkdir(parents=True, exist_ok=True)


def command_init(args: argparse.Namespace) -> int:
    root_path = Path(args.root).expanduser().resolve()
    sky = skyline_dir(root_path)
    sky.mkdir(parents=True, exist_ok=True)
    source_root = Path(args.source_root).expanduser().resolve() if args.source_root is not None else None
    with locked(sky):
        ensure_layout(sky)
        cfg = load_config(sky)
        cfg["schema_version"] = SCHEMA_VERSION
        cfg["coordination_mode"] = "autonomous"
        cfg["claim_mode"] = "legacy_advisory"
        cfg["snapshot_mode"] = "live_delta"
        cfg.pop("automatic_triggering_enabled", None)
        if source_root is not None:
            cfg["source_root"] = str(source_root)
        if args.runtime_digest is not None:
            cfg["runtime_digest"] = args.runtime_digest.strip()
        if args.archive_after is not None:
            cfg["archive_after_seconds"] = args.archive_after
        if args.stale_after is not None:
            cfg["stale_after_seconds"] = args.stale_after
        if args.max_active_agents is not None:
            cfg["max_active_agents"] = max(1, min(128, args.max_active_agents))
        json_write(config_path(sky), cfg)

        # Keep a runnable coordinator inside the shared Skyline folder so every chat
        # can invoke the same script without depending on the installed skill path.
        local_script = deploy_runtime(sky)

        global_md = sky / "SKYLINE.md"
        if not global_md.exists():
            legacy_md = root_path / "SKYLINE.md"
            if source_root is not None and root_path != source_root and legacy_md.is_file():
                shutil.copy2(legacy_md, global_md)
            else:
                title = args.title or root_path.name or "Skyline"
                global_md.write_text(
                    "# Skyline\n\n"
                    f"Coordination domain: {title}\n\n"
                    "## Objective\n"
                    "Describe the shared objective here.\n\n"
                    "## Global constraints\n"
                    "- Permission profile: unlimited, subject to platform/tool constraints.\n"
                    "- Divisions are context hints, not access boundaries.\n"
                    "- Agent workspaces are scratch/coordination areas, not sandboxes.\n"
                    "- Workers decide their own state, intent, methods, scope, and next action.\n"
                    "- Jobs are optional opportunity hints, never assignments; legacy claims are collision metadata only.\n"
                    "- Do not create work merely to keep a worker alive or satisfy a time quota.\n"
                    "- Use as many or as few workers as are materially useful; avoid redundant parallelism.\n"
                    "- Read changed coordination state, not unchanged history, unless a concrete question requires it.\n"
                    "- Skyline never creates scheduled automations, reminders, daemons, or self-launched chats.\n"
                    "- Workers may freely use any relevant Skill, connector, or normal tool, including Polaris.\n",
                    encoding="utf-8",
                )
    command_maintain(argparse.Namespace(root=args.root, quiet=True))
    emit({"skyline": str(sky), "runtime": str(local_script), "config": cfg, "global_context": str(global_md)})
    return 0


def division_dir(sky: Path, name: str) -> Path:
    return sky / "divisions" / validate_division(name)


def ensure_division(sky: Path, name: str, context: str | None = None) -> Path:
    name = validate_division(name)
    path = division_dir(sky, name)
    for rel in ("findings", "decisions", "inbox"):
        (path / rel).mkdir(parents=True, exist_ok=True)
    ctx = path / "CONTEXT.md"
    if not ctx.exists():
        ctx.write_text(
            f"# Division: {name}\n\n"
            "This division is a context-priority hint only. Agents may freely cross-reference other divisions and the real workspace.\n\n"
            "## Context\n"
            f"{context.strip() if context else 'Add division-specific context here.'}\n",
            encoding="utf-8",
        )
    elif context:
        ctx.write_text(
            f"# Division: {name}\n\n"
            "This division is a context-priority hint only. Agents may freely cross-reference other divisions and the real workspace.\n\n"
            "## Context\n"
            f"{context.strip()}\n",
            encoding="utf-8",
        )
    return path


def command_division_add(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        maintain_locked(sky)
        path = ensure_division(sky, args.name, args.context)
    emit({"division": path.name, "path": str(path), "context": str(path / "CONTEXT.md")})
    return 0


def job_locations(sky: Path) -> list[tuple[str, Path]]:
    return [(name, sky / "jobs" / name) for name in ("pending", "active", "review", "done", "archive")]


def find_job(sky: Path, job_id: str) -> tuple[str, Path, dict[str, Any]]:
    job_id = validate_id(job_id, "job id")
    for state, directory in job_locations(sky):
        path = directory / f"{job_id}.json"
        if path.exists():
            return state, path, json_read(path, {})
    raise SkylineError(f"unknown job: {job_id}")


def job_exists(sky: Path, job_id: str) -> bool:
    try:
        find_job(sky, job_id)
        return True
    except SkylineError:
        return False


def current_job_by_agent(sky: Path, agent_id: str) -> tuple[str, Path, dict[str, Any]] | None:
    for directory_name in ("active",):
        directory = sky / "jobs" / directory_name
        for path in directory.glob("*.json"):
            job = json_read(path, {})
            if job.get("assigned_agent") == agent_id:
                return directory_name, path, job
    return None


def new_job_id(title: str) -> str:
    return f"{slugify(title, 36)}-{secrets.token_hex(3)}"


def normalize_dependencies(values: list[str] | None) -> list[str]:
    if not values:
        return []
    return [validate_id(v, "dependency job id") for v in values]


def normalize_related(values: list[str] | None) -> list[str]:
    return [validate_division(v) for v in (values or [])]


def add_job_locked(
    sky: Path,
    *,
    job_id: str,
    title: str,
    instructions: str,
    kind: str = "WORK",
    role_hint: str = "generalist",
    division: str = "general",
    related: list[str] | None = None,
    priority: int = 100,
    depends_on: list[str] | None = None,
    validation: str = "none",
    ownership: list[str] | None = None,
    created_by: str = "constructor",
    target_job: str | None = None,
) -> dict[str, Any]:
    job_id = validate_id(job_id, "job id")
    if job_exists(sky, job_id):
        raise SkylineError(f"job already exists: {job_id}")
    division = validate_division(division)
    ensure_division(sky, division)
    related = normalize_related(related)
    for rel in related:
        ensure_division(sky, rel)
    deps = normalize_dependencies(depends_on)
    now = iso_now()
    job = {
        "schema_version": SCHEMA_VERSION,
        "id": job_id,
        "title": title.strip(),
        "kind": kind.upper(),
        "role_hint": role_hint.strip() or "generalist",
        "primary_division": division,
        "related_divisions": related,
        "priority": int(priority),
        "depends_on": deps,
        "validation": validation.lower(),
        "target_job": target_job,
        "ownership": ownership or [],
        "instructions": instructions.strip(),
        "status": "PENDING",
        "assigned_agent": None,
        "created_by": created_by,
        "created_at": now,
        "updated_at": now,
        "attempt": 0,
        "result_summary": None,
        "completed_by": None,
        "last_validation": None,
        "wait_reason": None,
        "wake_condition": None,
        "claim_id": None,
        "claim_generation": 0,
        "lease_expires_at": None,
    }
    json_write(sky / "jobs" / "pending" / f"{job_id}.json", job)
    return job


def command_job_add(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        maintain_locked(sky)
        job_id = args.id or new_job_id(args.title)
        job = add_job_locked(
            sky,
            job_id=job_id,
            title=args.title,
            instructions=args.instructions,
            kind=args.kind,
            role_hint=args.role_hint,
            division=args.division,
            related=args.related,
            priority=args.priority,
            depends_on=args.depends_on,
            validation=args.validation,
            ownership=args.ownership,
            created_by=args.created_by,
            target_job=args.target_job,
        )
    emit(job)
    return 0


def create_agent_locked(sky: Path) -> tuple[str, Path, dict[str, Any]]:
    for _ in range(20):
        stamp = utcnow().strftime("%Y%m%d-%H%M%S")
        agent_id = f"agent-{stamp}-{secrets.token_hex(3)}"
        path = sky / "agents" / agent_id
        try:
            path.mkdir()
            break
        except FileExistsError:
            continue
    else:
        raise SkylineError("could not allocate agent id")
    for rel in ("tmp", "inbox"):
        (path / rel).mkdir(parents=True, exist_ok=True)
    now = iso_now()
    state = {
        "schema_version": SCHEMA_VERSION,
        "agent_id": agent_id,
        "status": "ASSESSING",
        "assessment": "",
        "intent": "",
        "intent_scope": [],
        "associated_job": None,
        "decision_basis": "",
        "current_job": None,
        "current_claim_id": None,
        "claim_generation": None,
        "role_hint": None,
        "primary_division": None,
        "last_heartbeat": now,
        "last_inbox_check": now,
        "next_action": "choose and perform the highest-value useful action; coordinate only on meaningful overlap",
        "wait_kind": None,
        "wake_condition": None,
        "_coordination_fingerprints": {},
        "created_at": now,
        "updated_at": now,
    }
    save_agent(path, state)
    write_intent_from_agent(sky, state)
    save_agent(path, state)
    return agent_id, path, state

def load_agent(sky: Path, agent_id: str) -> tuple[Path, dict[str, Any]]:
    agent_id = validate_id(agent_id, "agent id")
    path = sky / "agents" / agent_id
    if not path.exists():
        raise SkylineError(f"unknown active agent: {agent_id}")
    state = json_read(path / "state.json", {})
    return path, state


def live_agent_index_path(sky: Path) -> Path:
    return sky / ".live-agents.json"


def update_live_agent_index_locked(sky: Path, state: dict[str, Any]) -> None:
    agent_id = str(state.get("agent_id") or "")
    if not agent_id:
        return
    path = live_agent_index_path(sky)
    record = json_read(path, None)
    existing = record.get("agents") if isinstance(record, dict) else None
    if isinstance(existing, list):
        ids = {str(value) for value in existing if value}
    else:
        cfg = load_config(sky)
        now = utcnow()
        ids = set()
        for directory in (sky / "agents").iterdir() if (sky / "agents").exists() else []:
            state_path = directory / "state.json"
            existing_state = json_read(state_path, None)
            if isinstance(existing_state, dict) and existing_state.get("agent_id") and agent_is_live(existing_state, cfg, now):
                ids.add(str(existing_state["agent_id"]))
    before = set(ids)
    if str(state.get("status") or "").upper() in INACTIVE_AGENT_STATUSES:
        ids.discard(agent_id)
    else:
        ids.add(agent_id)
    if ids != before or not isinstance(record, dict):
        json_write(path, {"schema_version": SCHEMA_VERSION, "agents": sorted(ids), "updated_at": iso_now()})


def save_agent(path: Path, state: dict[str, Any]) -> None:
    state["updated_at"] = iso_now()
    json_write(path / "state.json", state)
    if path.parent.name == "agents":
        update_live_agent_index_locked(path.parent.parent, state)


def live_agent_states_locked(sky: Path) -> tuple[list[dict[str, Any]], int]:
    cfg = load_config(sky)
    now = utcnow()
    index_path = live_agent_index_path(sky)
    record = json_read(index_path, None)
    indexed = record.get("agents") if isinstance(record, dict) else None
    bootstrap = not isinstance(indexed, list)
    if bootstrap:
        candidate_ids = [path.name for path in (sky / "agents").iterdir() if path.is_dir()] if (sky / "agents").exists() else []
    else:
        candidate_ids = [str(value) for value in indexed if value]

    live: list[dict[str, Any]] = []
    kept: list[str] = []
    pruned = 0
    for agent_id in candidate_ids:
        state_path = sky / "agents" / agent_id / "state.json"
        state = json_read(state_path, None)
        if isinstance(state, dict) and state.get("agent_id") and agent_is_live(state, cfg, now):
            live.append(state)
            kept.append(str(state["agent_id"]))
        else:
            pruned += 1

    normalized = sorted(set(kept))
    previous = sorted({str(value) for value in indexed if value}) if isinstance(indexed, list) else None
    if bootstrap or normalized != previous:
        json_write(index_path, {"schema_version": SCHEMA_VERSION, "agents": normalized, "updated_at": iso_now()})
    return live, pruned

def intent_path(sky: Path, agent_id: str) -> Path:
    return sky / "intents" / f"{validate_id(agent_id, 'agent id')}.json"


def intent_semantics(agent: dict[str, Any]) -> dict[str, Any]:
    return {
        "agent_id": agent.get("agent_id"),
        "agent_name": agent.get("display_name") or agent.get("agent_name"),
        "status": agent.get("status"),
        "assessment": agent.get("assessment", ""),
        "intent": agent.get("intent", ""),
        "intent_scope": agent.get("intent_scope", []),
        "associated_job": agent.get("associated_job") or agent.get("current_job"),
        "next_action": agent.get("next_action"),
        "decision_basis": agent.get("decision_basis", ""),
    }


def write_intent_from_agent(sky: Path, agent: dict[str, Any]) -> dict[str, Any]:
    path = intent_path(sky, str(agent["agent_id"]))
    semantic = intent_semantics(agent)
    existing = json_read(path, None)
    if isinstance(existing, dict) and all(existing.get(key) == value for key, value in semantic.items()):
        if existing.get("updated_at"):
            agent["intent_updated_at"] = existing["updated_at"]
        return existing

    now = iso_now()
    record = {"schema_version": SCHEMA_VERSION, **semantic, "updated_at": now}
    agent["intent_updated_at"] = now
    json_write(path, record)
    return record


def stable_digest(value: Any) -> str:
    import hashlib
    raw = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), default=str).encode("utf-8")
    return hashlib.sha256(raw).hexdigest()


def mission_digest(sky: Path) -> str:
    path = sky / "SKYLINE.md"
    try:
        return stable_digest(path.read_text(encoding="utf-8"))
    except OSError:
        return stable_digest("")


def agent_is_live(state: dict[str, Any], cfg: dict[str, Any], now: datetime | None = None) -> bool:
    if str(state.get("status") or "").upper() in INACTIVE_AGENT_STATUSES:
        return False
    now = now or utcnow()
    heartbeat = parse_time(state.get("last_heartbeat")) or parse_time(state.get("updated_at"))
    return heartbeat is not None and (now - heartbeat).total_seconds() <= int(cfg["stale_after_seconds"])


def live_agent_ids_locked(sky: Path) -> list[str]:
    states, _ = live_agent_states_locked(sky)
    return [str(state["agent_id"]) for state in states if state.get("agent_id")]


def compact_agent_state(state: dict[str, Any]) -> dict[str, Any]:
    return {
        "agent_id": state.get("agent_id"),
        "display_name": state.get("display_name") or state.get("agent_name"),
        "status": state.get("status"),
        "assessment": state.get("assessment", ""),
        "intent": state.get("intent", ""),
        "intent_scope": state.get("intent_scope", []),
        "associated_job": state.get("associated_job") or state.get("current_job"),
        "next_action": state.get("next_action"),
        "updated_at": state.get("intent_updated_at") or state.get("created_at"),
    }


def live_peer_intents_locked(sky: Path, agent_id: str) -> list[dict[str, Any]]:
    cfg = load_config(sky)
    peers = []
    states, _ = live_agent_states_locked(sky)
    for state in states:
        peer_id = str(state.get("agent_id") or "")
        if not peer_id or peer_id == agent_id:
            continue
        intent = json_read(intent_path(sky, peer_id), None)
        peers.append(intent if isinstance(intent, dict) else compact_agent_state(state))
    peers.sort(key=lambda row: str(row.get("updated_at") or ""), reverse=True)
    return peers[: int(cfg.get("max_peer_intents", DEFAULT_MAX_PEERS))]


def opportunity_revision_signature(sky: Path) -> str:
    marker = sky / ".opportunity-revision"
    try:
        marker_value = marker.read_text(encoding="ascii").strip()
    except OSError:
        marker_value = ""
    parts: list[Any] = [marker_value]
    for state_name in CURRENT_OPPORTUNITY_STATES:
        directory = sky / "jobs" / state_name
        try:
            stat = directory.stat()
            parts.append((state_name, stat.st_mtime_ns))
        except OSError:
            parts.append((state_name, 0))
    return stable_digest(parts)


def compact_opportunity(job: dict[str, Any], state_name: str) -> dict[str, Any]:
    return {
        "id": job.get("id"),
        "title": job.get("title"),
        "kind": job.get("kind"),
        "status": job.get("status") or state_name.upper(),
        "assigned_agent": job.get("assigned_agent"),
        "ownership": job.get("ownership", []),
        "priority": job.get("priority", 100),
        "updated_at": job.get("updated_at"),
    }


def opportunity_snapshot_locked(sky: Path) -> dict[str, Any]:
    cfg = load_config(sky)
    limit = int(cfg.get("max_opportunities", DEFAULT_MAX_OPPORTUNITIES))
    revision = opportunity_revision_signature(sky)
    cache_path = sky / ".opportunity-cache.json"
    cached = json_read(cache_path, None)
    if isinstance(cached, dict) and cached.get("revision") == revision and int(cached.get("limit") or 0) == limit:
        return cached

    by_state: dict[str, list[dict[str, Any]]] = {}
    counts: dict[str, int] = {}
    merged: list[dict[str, Any]] = []
    for state_name in CURRENT_OPPORTUNITY_STATES:
        rows: list[dict[str, Any]] = []
        for path in (sky / "jobs" / state_name).glob("*.json"):
            job = json_read(path, {})
            if isinstance(job, dict):
                rows.append(compact_opportunity(job, state_name))
        rows.sort(key=lambda row: (int(row.get("priority") or 100), str(row.get("updated_at") or ""), str(row.get("id") or "")))
        counts[state_name] = len(rows)
        by_state[state_name] = rows[:limit]
        merged.extend(rows[:limit])
    merged.sort(key=lambda row: (int(row.get("priority") or 100), str(row.get("updated_at") or ""), str(row.get("id") or "")))
    snapshot = {
        "schema_version": SCHEMA_VERSION,
        "revision": revision,
        "limit": limit,
        "counts": counts,
        "by_state": by_state,
        "merged": merged[:limit],
        "updated_at": iso_now(),
    }
    json_write(cache_path, snapshot)
    return snapshot


def compact_opportunities_locked(sky: Path) -> list[dict[str, Any]]:
    return list(opportunity_snapshot_locked(sky).get("merged", []))


def recent_outcomes_locked(sky: Path) -> list[dict[str, Any]]:
    cfg = load_config(sky)
    limit = int(cfg.get("recent_outcomes", DEFAULT_RECENT_OUTCOMES))
    directory = sky / "outcomes"
    try:
        revision = str(directory.stat().st_mtime_ns)
    except OSError:
        revision = "0"
    cache_path = sky / ".outcome-cache.json"
    cached = json_read(cache_path, None)
    if isinstance(cached, dict) and cached.get("revision") == revision and int(cached.get("limit") or 0) == limit:
        rows = cached.get("rows")
        if isinstance(rows, list):
            return rows

    rows = [json_read(path, {}) for path in directory.glob("*.json")]
    rows = [row for row in rows if isinstance(row, dict)]
    rows.sort(key=lambda row: str(row.get("created_at") or ""), reverse=True)
    rows = rows[:limit]
    json_write(cache_path, {"schema_version": SCHEMA_VERSION, "revision": revision, "limit": limit, "rows": rows, "updated_at": iso_now()})
    return rows


def open_whiteboard_cards_locked(sky: Path) -> list[dict[str, Any]]:
    rows = []
    for path in (sky / "whiteboard" / "cards").glob("*.json"):
        card = json_read(path, {})
        if str(card.get("status", "open")).lower() in {"open", "active"}:
            rows.append(card)
    rows.sort(key=lambda row: str(row.get("updated_at") or row.get("created_at") or ""), reverse=True)
    return rows[:8]


def active_test_runs_locked(sky: Path) -> list[dict[str, Any]]:
    rows = [json_read(path, {}) for path in (sky / "test-runs" / "active").glob("*.json")]
    rows.sort(key=lambda row: str(row.get("updated_at") or row.get("started_at") or ""), reverse=True)
    return rows[:8]


def coordination_objects_locked(sky: Path, agent_id: str) -> dict[str, Any]:
    cfg = load_config(sky)
    objects: dict[str, Any] = {}
    for row in live_peer_intents_locked(sky, agent_id):
        objects["intent:" + str(row.get("agent_id"))] = row
    for row in compact_opportunities_locked(sky):
        objects["opportunity:" + str(row.get("id"))] = row
    for row in recent_outcomes_locked(sky):
        objects["outcome:" + str(row.get("id"))] = row
    for row in open_whiteboard_cards_locked(sky):
        objects["whiteboard:" + str(row.get("id"))] = row
    for row in active_test_runs_locked(sky):
        objects["test_run:" + str(row.get("id"))] = row
    objects["meta:mission"] = {"path": str(sky / "SKYLINE.md"), "digest": mission_digest(sky), "schema_version": SCHEMA_VERSION, "coordination_mode": cfg.get("coordination_mode"), "claim_mode": cfg.get("claim_mode"), "source_root": cfg.get("source_root")}
    return objects


def coordination_delta_locked(sky: Path, agent: dict[str, Any], force_full: bool = False) -> dict[str, Any]:
    objects = coordination_objects_locked(sky, str(agent.get("agent_id") or ""))
    current = {key: stable_digest(value) for key, value in objects.items()}
    previous = agent.get("_coordination_fingerprints")
    bootstrap = force_full or not isinstance(previous, dict) or not previous
    changed = sorted(objects) if bootstrap else sorted(key for key, digest in current.items() if previous.get(key) != digest)
    removed = [] if bootstrap else sorted(key for key in previous if key not in current)
    changes = {"peer_intents": [], "opportunities": [], "outcomes": [], "whiteboard_cards": [], "test_runs": [], "meta": []}
    buckets = {"intent": "peer_intents", "opportunity": "opportunities", "outcome": "outcomes", "whiteboard": "whiteboard_cards", "test_run": "test_runs", "meta": "meta"}
    for key in changed:
        changes[buckets.get(key.split(":", 1)[0], "meta")].append(objects[key])
    agent["_coordination_fingerprints"] = current
    agent["_coordination_seen_at"] = iso_now()
    return {"mode": "bootstrap" if bootstrap else "delta", "cursor": stable_digest(current)[:16], "changed_count": len(changed), "removed": removed, "changes": changes}


def team_summary_locked(sky: Path) -> dict[str, Any]:
    cfg = load_config(sky)
    states, stale = live_agent_states_locked(sky)
    live = [compact_agent_state(state) for state in states]
    scopes: list[tuple[str, str]] = []
    for state in live:
        for scope in state.get("intent_scope") or []:
            value = str(scope).strip()
            if value:
                scopes.append((value, str(state.get("agent_id"))))

    collisions: list[dict[str, Any]] = []
    seen_pairs: set[tuple[tuple[str, str], tuple[str, str]]] = set()
    for index, (left_scope, left_agent) in enumerate(scopes):
        for right_scope, right_agent in scopes[index + 1 :]:
            if left_agent == right_agent or not scopes_overlap(left_scope, right_scope):
                continue
            pair = tuple(sorted(((left_scope, left_agent), (right_scope, right_agent))))
            if pair in seen_pairs:
                continue
            seen_pairs.add(pair)
            collisions.append(
                {
                    "scope": pair[0][0],
                    "other_scope": pair[1][0],
                    "agents": sorted({pair[0][1], pair[1][1]}),
                }
            )
    collisions.sort(key=lambda row: (str(row["scope"]), str(row["other_scope"]), row["agents"]))
    opportunity_counts = dict(opportunity_snapshot_locked(sky).get("counts", {}))
    return {"live_workers": len(live), "stale_workers": stale, "independent_declared_scopes": len({scope for scope, _ in scopes}), "scope_collision_count": len(collisions), "scope_collisions": collisions[:4], "opportunity_counts": opportunity_counts, "active_test_runs": len(list((sky / "test-runs" / "active").glob("*.json"))), "stale_after_seconds": int(cfg["stale_after_seconds"])}


def _scope_prefix(scope: str) -> str:
    """Return the stable prefix used for conservative advisory overlap checks."""
    normalized = scope.strip().replace("\\", "/").rstrip("/")
    if not normalized:
        return ""
    wildcard = normalized.find("*")
    if wildcard >= 0:
        normalized = normalized[:wildcard]
    return normalized.rstrip("/")


def _prefix_contains(prefix: str, value: str) -> bool:
    if not prefix:
        return True
    if value == prefix:
        return True
    return value.startswith(prefix) and value[len(prefix) :].startswith("/")


def scopes_overlap(left: str, right: str) -> bool:
    """Detect exact, path-prefix, and wildcard-like scope overlap.

    Scope declarations are advisory labels rather than filesystem globs. A
    conservative prefix check catches common declarations such as
    ``src/guidance/*`` versus ``src/guidance/entry.rs`` while preserving the
    useful distinction between ``src/foo`` and ``src/foobar``.
    """
    left = left.strip().replace("\\", "/").rstrip("/")
    right = right.strip().replace("\\", "/").rstrip("/")
    if left == right:
        return True
    return _prefix_contains(_scope_prefix(left), right) or _prefix_contains(_scope_prefix(right), left)


def message_needs_attention(message: dict[str, Any]) -> bool:
    return str(message.get("type") or "").upper() in {"ALERT", "BLOCKER", "DECISION_PROPOSAL", "REVIEW_REQUEST"} or bool(message.get("requires_response")) or bool(message.get("user_relayed"))


def coordination_messages_locked(sky: Path, agent: dict[str, Any], bootstrap: bool = False, full: bool = False) -> dict[str, Any]:
    cfg = load_config(sky)
    now = utcnow()
    since = now - timedelta(seconds=int(cfg.get("bootstrap_broadcast_history_seconds", DEFAULT_BOOTSTRAP_BROADCAST_HISTORY_SECONDS))) if bootstrap else parse_time(agent.get("last_inbox_check"))
    rows = []
    for path in collect_inbox_paths(sky, agent):
        if since:
            try:
                if datetime.fromtimestamp(path.stat().st_mtime, timezone.utc) <= since:
                    continue
            except OSError:
                pass
        message = json_read(path, {})
        created = parse_time(message.get("created_at"))
        if created and created > now:
            continue
        if since and created and created <= since:
            continue
        rows.append(message)
    rows.sort(key=lambda row: str(row.get("created_at") or ""), reverse=True)
    attention = [row for row in rows if message_needs_attention(row)]
    routine = [row for row in rows if not message_needs_attention(row)]
    selected = attention + routine if full else (attention + routine)[: int(cfg.get("max_messages", DEFAULT_MAX_MESSAGES))]
    agent["last_inbox_check"] = now.isoformat()
    return {"unread_count": len(rows), "attention_count": len(attention), "deferred_routine_count": max(0, len(rows) - len(selected)), "attention_required": bool(attention), "delta_mode": "bootstrap" if bootstrap else "unread", "unread_messages": selected}


def record_outcome_locked(sky: Path, agent: dict[str, Any], summary: str, related_job: str | None = None) -> dict[str, Any]:
    now = iso_now()
    oid = f"{utcnow().strftime('%Y%m%d-%H%M%S')}-{slugify(str(agent['agent_id']), 24)}-{secrets.token_hex(3)}"
    record = {
        "schema_version": SCHEMA_VERSION,
        "id": oid,
        "agent_id": agent.get("agent_id"),
        "agent_name": agent.get("display_name") or agent.get("agent_name"),
        "summary": summary.strip(),
        "related_job": related_job or agent.get("associated_job") or agent.get("current_job"),
        "intent": agent.get("intent", ""),
        "assessment": agent.get("assessment", ""),
        "created_at": now,
    }
    json_write(sky / "outcomes" / f"{oid}.json", record)
    agent["last_outcome"] = record
    return record


def model_ready_payload_locked(sky: Path, agent_path: Path, agent: dict[str, Any], *, status: str = "READY_TO_DECIDE", force_full: bool = False, bootstrap: bool = False) -> dict[str, Any]:
    cfg = load_config(sky)
    delta = coordination_delta_locked(sky, agent, force_full)
    coordination = coordination_messages_locked(sky, agent, bootstrap=bootstrap, full=force_full)
    save_agent(agent_path, agent)
    return {
        "status": status,
        "agent_id": agent.get("agent_id"),
        "display_name": agent.get("display_name") or agent.get("agent_name"),
        "agent": compact_agent_state(agent),
        "team_summary": team_summary_locked(sky),
        "coordination_delta": delta,
        "coordination": coordination,
        "mission": str(sky / "SKYLINE.md"),
        "mission_digest": mission_digest(sky),
        "source_root": cfg.get("source_root"),
        "scratch": str(agent_path / "tmp"),
        "instruction": "Choose and execute the highest-value useful action autonomously. Shared state is evidence, not an assignment. Coordinate only for meaningful overlap or exclusive resources; do not poll unchanged state, duplicate healthy work, or remain active to satisfy a time quota.",
    }




def reactivate_agent_for_autonomy(agent: dict[str, Any]) -> None:
    status = str(agent.get("status") or "").upper()
    if status in INACTIVE_AGENT_STATUSES or status in {"WAITING", "BLOCKED", "LOST_CLAIM"}:
        agent["status"] = "ASSESSING"
    if status == "STALE_LEGACY" or str(agent.get("wait_kind") or "").upper() == "SCHEDULER":
        agent["wait_kind"] = None
        agent["wake_condition"] = None
    next_action = str(agent.get("next_action") or "")
    lowered = next_action.lower()
    if (
        not next_action
        or lowered.startswith("wait for a runnable")
        or "claim the next useful job" in lowered
        or "refresh this chat before treating it as live" in lowered
    ):
        agent["next_action"] = "choose and perform the highest-value useful action; coordinate only on meaningful overlap"


def command_next(args: argparse.Namespace) -> int:
    """Refresh only live/changed shared state; the model chooses what to do next."""
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        ensure_layout(sky)
        maybe_maintain_locked(sky)
        is_new = not bool(args.agent)
        if args.agent:
            agent_path, agent = load_agent(sky, args.agent)
        else:
            _, agent_path, agent = create_agent_locked(sky)
        if getattr(args, "job", None):
            find_job(sky, args.job)
            agent["associated_job"] = args.job
        reactivate_agent_for_autonomy(agent)
        agent["last_heartbeat"] = iso_now()
        write_intent_from_agent(sky, agent)
        payload = model_ready_payload_locked(sky, agent_path, agent, status="OBSERVE", force_full=bool(getattr(args, "full", False)) or is_new, bootstrap=is_new)
    emit(payload)
    return 0

def command_seat(args: argparse.Namespace) -> int:
    sky = find_skyline(args.start)
    if sky is None:
        emit({
            "status": "NO_SKYLINE",
            "start": str(Path(args.start).expanduser().resolve()),
            "reason": "no initialized skyline/ found in this directory or its ancestors",
        })
        return 3
    require_initialized(sky)
    runtime = deploy_runtime(sky)
    with locked(sky):
        ensure_layout(sky)
        maybe_maintain_locked(sky)
        is_new = not bool(args.agent)
        if args.agent:
            agent_path, agent = load_agent(sky, args.agent)
        else:
            _, agent_path, agent = create_agent_locked(sky)
        agent.setdefault("assessment", "")
        agent.setdefault("intent", "")
        agent.setdefault("intent_scope", [])
        agent.setdefault("associated_job", agent.get("current_job"))
        agent.setdefault("decision_basis", "")
        reactivate_agent_for_autonomy(agent)
        agent["last_heartbeat"] = iso_now()
        write_intent_from_agent(sky, agent)
        payload = model_ready_payload_locked(sky, agent_path, agent, force_full=bool(getattr(args, "full", False)) or is_new, bootstrap=is_new)
        payload["skyline"] = str(sky)
        payload["runtime"] = str(runtime)
    emit(payload)
    return 0

def command_start(args: argparse.Namespace) -> int:
    """High-level worker entrypoint: load shared state, never auto-assign work."""
    return command_seat(args)


def command_sync(args: argparse.Namespace) -> int:
    """Persist model-authored state/intent, read messages, and return peer/team state."""
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        ensure_layout(sky)
        maybe_maintain_locked(sky)
        agent_path, agent = load_agent(sky, args.agent)
        claim_status = None
        claim_id = getattr(args, "claim", None)
        if claim_id:
            try:
                a_path, a_state, job_path, job, record = verify_claim_locked(sky, args.agent, claim_id)
                renew_claim_locked(sky, a_path, a_state, job_path, job, record)
                agent = a_state
                claim_status = {"status": "VALID_ADVISORY", "job_id": job.get("id"), "claim_id": claim_id}
            except SkylineError as exc:
                claim_status = {"status": "STALE_ADVISORY", "claim_id": claim_id, "warning": str(exc)}

        state = getattr(args, "state", None) or getattr(args, "status", None)
        if state:
            agent["status"] = str(state).strip()
        assessment = getattr(args, "assessment", None)
        if assessment is not None:
            agent["assessment"] = assessment.strip()
        intent = getattr(args, "intent", None)
        if intent is not None:
            agent["intent"] = intent.strip()
        scopes = getattr(args, "scope", None)
        if scopes is not None:
            agent["intent_scope"] = [str(v).strip() for v in scopes if str(v).strip()]
        decision_basis = getattr(args, "decision_basis", None)
        if decision_basis is not None:
            agent["decision_basis"] = decision_basis.strip()
        next_action = getattr(args, "next_action", None)
        if next_action is not None:
            agent["next_action"] = next_action.strip()
        job_id = getattr(args, "job", None)
        if job_id:
            find_job(sky, job_id)
            agent["associated_job"] = job_id
        if getattr(args, "clear_job", False):
            agent["associated_job"] = None
        wait_kind = getattr(args, "wait_kind", None)
        if wait_kind is not None:
            agent["wait_kind"] = wait_kind
        wake_condition = getattr(args, "wake_condition", None)
        if wake_condition is not None:
            agent["wake_condition"] = wake_condition

        note = getattr(args, "note", None)
        if note:
            journal = list(agent.get("journal", []))
            journal.append({"at": iso_now(), "note": note.strip()})
            agent["journal"] = journal[-30:]
            associated = agent.get("associated_job") or agent.get("current_job")
            if associated:
                try:
                    _, jpath, job = find_job(sky, str(associated))
                    notes = list(job.get("progress_notes", []))
                    notes.append({"at": iso_now(), "agent": args.agent, "note": note.strip(), "coordination": "model_intent"})
                    job["progress_notes"] = notes[-30:]
                    job["updated_at"] = iso_now()
                    json_write(jpath, job)
                except SkylineError:
                    pass

        agent["last_heartbeat"] = iso_now()
        intent_record = write_intent_from_agent(sky, agent)

        delta = coordination_delta_locked(sky, agent, bool(getattr(args, "full", False)))
        coordination = coordination_messages_locked(sky, agent, full=bool(getattr(args, "full", False)))
        save_agent(agent_path, agent)
        summary = team_summary_locked(sky)

    emit({
        "status": "SYNCED",
        "agent": compact_agent_state(agent),
        "intent": intent_record,
        "messages": coordination["unread_messages"],
        "team_summary": summary,
        "coordination_delta": delta,
        "coordination": coordination,
        "advisory_claim": claim_status,
        "instruction": "Act on material changes only. If nothing relevant changed, continue current work without another coordination round-trip.",
    })
    return 0

def command_guard(args: argparse.Namespace) -> int:
    """Report legacy claim health as advisory collision metadata."""
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        maintain_locked(sky)
        try:
            agent_path, agent, job_path, job, record = verify_claim_locked(sky, args.agent, args.claim)
            if args.renew:
                renew_claim_locked(sky, agent_path, agent, job_path, job, record)
                record = json_read(claim_path(sky, job["id"]), record)
            payload = {
                "status": "VALID_ADVISORY",
                "agent_id": args.agent,
                "job_id": job["id"],
                "claim_id": args.claim,
                "lease_expires_at": record.get("lease_expires_at"),
                "note": "This is a collision hint only; it does not authorize or prohibit unrelated work.",
            }
        except SkylineError as exc:
            payload = {
                "status": "STALE_ADVISORY",
                "agent_id": args.agent,
                "claim_id": args.claim,
                "warning": str(exc),
                "note": "Reassess conflicting mutation if relevant; continue safe useful work based on mission and peer intent.",
            }
    emit(payload)
    return 0

def command_update(args: argparse.Namespace) -> int:
    """Compatibility alias for model-authored sync/update state."""
    ns = argparse.Namespace(**vars(args))
    ns.state = getattr(args, "status", None)
    ns.assessment = getattr(args, "assessment", None)
    ns.intent = getattr(args, "intent", None)
    ns.scope = getattr(args, "scope", None)
    ns.decision_basis = getattr(args, "decision_basis", None)
    ns.job = getattr(args, "job", None)
    ns.clear_job = getattr(args, "clear_job", False)
    return command_sync(ns)

def move_job(path: Path, destination: Path, job: dict[str, Any]) -> Path:
    destination.parent.mkdir(parents=True, exist_ok=True)
    json_write(destination, job)
    path.unlink(missing_ok=True)
    return destination


def create_validation_job_locked(sky: Path, target: dict[str, Any]) -> dict[str, Any]:
    base = f"verify-{target['id']}"
    job_id = base
    n = 2
    while job_exists(sky, job_id):
        job_id = f"{base}-{n}"
        n += 1
    return add_job_locked(
        sky,
        job_id=job_id,
        title=f"Validate: {target['title']}",
        instructions=(
            f"Independently validate job {target['id']}. Inspect actual outputs, tests, logs, diffs, or evidence. "
            "Do not trust the worker summary alone. Finish by running `skyline.py validate` with PASS, FAIL, or INCONCLUSIVE."
        ),
        kind="VERIFY",
        role_hint="independent validator",
        division=target.get("primary_division", "general"),
        related=target.get("related_divisions", []),
        priority=max(0, int(target.get("priority", 100)) - 10),
        depends_on=[],
        validation="none",
        ownership=[],
        created_by="skyline-validation",
        target_job=target["id"],
    )


def finish_agent_job(sky: Path, agent_id: str, claim_id: str, result_summary: str, force_validation: bool | None = None) -> dict[str, Any]:
    agent_path, agent, job_path, job, _ = verify_claim_locked(sky, agent_id, claim_id)
    if job.get("kind") == "VERIFY":
        raise SkylineError("validation jobs must finish with `skyline.py validate`, not `complete`")
    job["result_summary"] = result_summary.strip()
    job["completed_by"] = agent_id
    job["updated_at"] = iso_now()
    job["assigned_agent"] = None
    release_claim_locked(sky, job)
    needs_validation = force_validation if force_validation is not None else job.get("validation") == "required"

    if needs_validation and job.get("kind") != "VERIFY":
        job["status"] = "READY_FOR_VERIFY"
        job["completed_at"] = iso_now()
        review_path = sky / "jobs" / "review" / f"{job['id']}.json"
        move_job(job_path, review_path, job)
        validation_job = create_validation_job_locked(sky, job)
        final_status = "READY_FOR_VERIFY"
    else:
        job["status"] = "COMPLETE"
        job["completed_at"] = iso_now()
        done_path = sky / "jobs" / "done" / f"{job['id']}.json"
        move_job(job_path, done_path, job)
        validation_job = None
        final_status = "COMPLETE"

    agent["status"] = "REASSESSING"
    agent["current_job"] = None
    agent["current_claim_id"] = None
    agent["claim_generation"] = None
    if agent.get("associated_job") == job.get("id"):
        agent["associated_job"] = None
    agent["last_heartbeat"] = iso_now()
    agent["next_action"] = "reassess mission, evidence, and peer intents; choose the next useful action"
    agent["wait_kind"] = None
    agent["wake_condition"] = None
    agent["role_hint"] = None
    agent["primary_division"] = None
    save_agent(agent_path, agent)
    write_intent_from_agent(sky, agent)
    return {"job": job, "status": final_status, "validation_job": validation_job, "agent": agent}


def command_complete(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        maintain_locked(sky)
        force = True if args.needs_validation else None
        result = finish_agent_job(sky, args.agent, args.claim, args.result, force)
    emit(result)
    return 0


def command_finish(args: argparse.Namespace) -> int:
    """Record a durable outcome; legacy claimed jobs may still be completed compatibly."""
    if not getattr(args, "claim", None):
        return command_outcome(argparse.Namespace(root=args.root, agent=args.agent, summary=args.result, job=getattr(args, "job", None)))

    sky = skyline_dir(args.root)
    require_initialized(sky)
    claim_valid = False
    kind = "WORK"
    with locked(sky):
        maintain_locked(sky)
        try:
            _, _, _, job, _ = verify_claim_locked(sky, args.agent, args.claim)
            kind = str(job.get("kind", "WORK")).upper()
            claim_valid = True
        except SkylineError:
            claim_valid = False

    if not claim_valid:
        return command_outcome(argparse.Namespace(root=args.root, agent=args.agent, summary=args.result, job=getattr(args, "job", None)))

    if kind == "VERIFY":
        if not args.validation_result:
            raise SkylineError("legacy VERIFY jobs require --validation-result PASS, FAIL, or INCONCLUSIVE")
        return command_validate(argparse.Namespace(
            root=args.root,
            agent=args.agent,
            claim=args.claim,
            result=args.validation_result,
            summary=args.result,
        ))

    return command_complete(argparse.Namespace(
        root=args.root,
        agent=args.agent,
        claim=args.claim,
        result=args.result,
        needs_validation=False,
    ))

def command_yield(args: argparse.Namespace) -> int:
    """Record a blocked dependency, release any legacy reservation, and reassess instead of idling."""
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        maintain_locked(sky)
        agent_path, agent = load_agent(sky, args.agent)
        released_job = None
        claim_id = getattr(args, "claim", None)
        if claim_id:
            try:
                _, _, job_path, job, _ = verify_claim_locked(sky, args.agent, claim_id)
                released_job = job.get("id")
                notes = list(job.get("progress_notes", []))
                notes.append({"at": iso_now(), "agent": args.agent, "note": f"DEPENDENCY/BLOCKER: {args.reason}"})
                job["progress_notes"] = notes[-30:]
                release_claim_locked(sky, job)
                job["status"] = "PENDING"
                job["assigned_agent"] = None
                job["wait_reason"] = args.reason
                job["wake_condition"] = args.wake_condition
                job["updated_at"] = iso_now()
                move_job(job_path, sky / "jobs" / "pending" / f"{job['id']}.json", job)
            except SkylineError:
                pass
        agent["status"] = "REASSESSING"
        agent["blocked_dependency"] = {"reason": args.reason, "wake_condition": args.wake_condition, "kind": args.kind}
        agent["current_job"] = None if released_job else agent.get("current_job")
        agent["current_claim_id"] = None if released_job else agent.get("current_claim_id")
        agent["claim_generation"] = None if released_job else agent.get("claim_generation")
        agent["next_action"] = "reassess mission and peer state; choose another useful action unless genuine external waiting is best"
        agent["last_heartbeat"] = iso_now()
        save_agent(agent_path, agent)
        write_intent_from_agent(sky, agent)
        payload = model_ready_payload_locked(sky, agent_path, agent, status="REASSESS")
        payload["released_legacy_job"] = released_job
    emit(payload)
    return 0

def command_pivot(args: argparse.Namespace) -> int:
    """Leave an optional handoff, then let the model choose its next direction."""
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        maintain_locked(sky)
        agent_path, agent = load_agent(sky, args.agent)
        old_job_id = agent.get("current_job") or agent.get("associated_job")
        claim_id = getattr(args, "claim", None)
        if claim_id:
            try:
                _, _, job_path, job, _ = verify_claim_locked(sky, args.agent, claim_id)
                old_job_id = job["id"]
                notes = list(job.get("progress_notes", []))
                notes.append({"at": iso_now(), "agent": args.agent, "note": f"HANDOFF: {args.note.strip()}"})
                job["progress_notes"] = notes[-30:]
                release_claim_locked(sky, job)
                job["status"] = "PENDING"
                job["assigned_agent"] = None
                job["updated_at"] = iso_now()
                move_job(job_path, sky / "jobs" / "pending" / f"{old_job_id}.json", job)
            except SkylineError:
                pass
        agent["status"] = "REASSESSING"
        agent["assessment"] = args.note.strip()
        agent["current_job"] = None
        agent["current_claim_id"] = None
        agent["claim_generation"] = None
        agent["associated_job"] = None
        if getattr(args, "to_job", None):
            find_job(sky, args.to_job)
            agent["associated_job"] = args.to_job
        agent["next_action"] = "choose the highest-value next action from current mission/evidence/peer state"
        agent["last_heartbeat"] = iso_now()
        save_agent(agent_path, agent)
        write_intent_from_agent(sky, agent)
        payload = model_ready_payload_locked(sky, agent_path, agent, status="REASSESS")
        payload["pivoted_from"] = old_job_id
        payload["handoff_note"] = args.note.strip()
    emit(payload)
    return 0

def command_wake(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        maintain_locked(sky)
        state, path, job = find_job(sky, args.job)
        if state != "pending":
            raise SkylineError(f"job {args.job} is not pending")
        job["status"] = "PENDING"
        job["wait_reason"] = None
        job["wake_condition"] = None
        job["updated_at"] = iso_now()
        json_write(path, job)
    emit(job)
    return 0


def command_validate(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    result = args.result.upper()
    with locked(sky):
        maintain_locked(sky)
        agent_path, agent, validator_path, validator_job, _ = verify_claim_locked(sky, args.agent, args.claim)
        if validator_job.get("kind") != "VERIFY" or not validator_job.get("target_job"):
            raise SkylineError("current job is not a validation job")
        target_id = validator_job["target_job"]
        target_path = sky / "jobs" / "review" / f"{target_id}.json"
        if not target_path.exists():
            raise SkylineError(f"validation target is not awaiting review: {target_id}")
        target = json_read(target_path, {})

        record = {
            "schema_version": SCHEMA_VERSION,
            "target_job": target_id,
            "validator_job": validator_job["id"],
            "validator_agent": args.agent,
            "validator_claim_id": args.claim,
            "result": result,
            "summary": args.summary.strip(),
            "created_at": iso_now(),
        }
        validation_path = sky / "validations" / f"{target_id}__{validator_job['id']}.json"
        json_write(validation_path, record)
        target["last_validation"] = record
        target["updated_at"] = iso_now()

        if result == "PASS":
            target["status"] = "COMPLETE"
            target["completed_at"] = iso_now()
            move_job(target_path, sky / "jobs" / "done" / f"{target_id}.json", target)
        elif result == "FAIL":
            target["status"] = "REVISION_REQUIRED"
            target["assigned_agent"] = None
            target["wait_reason"] = None
            target["wake_condition"] = None
            target["claim_id"] = None
            target["lease_expires_at"] = None
            move_job(target_path, sky / "jobs" / "pending" / f"{target_id}.json", target)
        else:
            target["status"] = "READY_FOR_VERIFY"
            json_write(target_path, target)
            create_validation_job_locked(sky, target)

        release_claim_locked(sky, validator_job)
        validator_job["status"] = "COMPLETE"
        validator_job["result_summary"] = f"{result}: {args.summary.strip()}"
        validator_job["completed_by"] = args.agent
        validator_job["assigned_agent"] = None
        validator_job["completed_at"] = iso_now()
        validator_job["updated_at"] = iso_now()
        move_job(validator_path, sky / "jobs" / "done" / f"{validator_job['id']}.json", validator_job)

        agent["status"] = "IDLE"
        agent["current_job"] = None
        agent["current_claim_id"] = None
        agent["claim_generation"] = None
        agent["last_heartbeat"] = iso_now()
        agent["next_action"] = "reassess mission, evidence, and peer state; choose the highest-value useful next action if any remains"
        agent["wait_kind"] = None
        agent["wake_condition"] = None
        save_agent(agent_path, agent)
    emit({"validation": record, "target": target, "validator_job": validator_job})
    return 0


def message_file(directory: Path, sender: str, subject: str) -> Path:
    stamp = utcnow().strftime("%Y%m%dT%H%M%S%fZ")
    name = f"{stamp}__{slugify(sender, 24)}__{slugify(subject, 40)}__{secrets.token_hex(3)}.json"
    directory.mkdir(parents=True, exist_ok=True)
    return directory / name


def command_send(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        sender = validate_id(args.agent, "agent id")
        _, sender_state = load_agent(sky, sender)
        recipients = []
        for target in args.to:
            if target.lower() == "all":
                recipients.append(("all", sky / "messages" / "all"))
            elif target.startswith("agent:"):
                aid = validate_id(target.split(":", 1)[1], "agent id")
                path, _ = load_agent(sky, aid)
                recipients.append((f"agent:{aid}", path / "inbox"))
            elif target.startswith("job:"):
                jid = validate_id(target.split(":", 1)[1], "job id")
                find_job(sky, jid)
                recipients.append((f"job:{jid}", sky / "messages" / "jobs" / jid))
            elif target.startswith("division:"):
                div = validate_division(target.split(":", 1)[1])
                path = ensure_division(sky, div)
                recipients.append((f"division:{div}", path / "inbox"))
            else:
                aid = validate_id(target, "agent id")
                path, _ = load_agent(sky, aid)
                recipients.append((f"agent:{aid}", path / "inbox"))

        created = []
        for label, directory in recipients:
            record = {
                "schema_version": SCHEMA_VERSION,
                "id": secrets.token_hex(12),
                "from": sender,
                "from_name": sender_state.get("display_name") or sender_state.get("agent_name"),
                "to": label,
                "type": args.type.upper(),
                "subject": args.subject.strip(),
                "body": args.body.strip(),
                "related": args.related or [],
                "requires_response": args.requires_response,
                "user_relayed": bool(getattr(args, "user_relayed", False)),
                "created_at": iso_now(),
            }
            path = message_file(directory, sender, args.subject)
            json_write(path, record)
            created.append(str(path))
    emit({"created": created})
    return 0


def collect_inbox_paths(sky: Path, agent: dict[str, Any]) -> list[Path]:
    agent_path = sky / "agents" / agent["agent_id"]
    paths = list((agent_path / "inbox").glob("*.json"))
    paths += list((sky / "messages" / "all").glob("*.json"))
    current_job = agent.get("associated_job") or agent.get("current_job")
    related_divisions: list[str] = []
    if current_job:
        paths += list((sky / "messages" / "jobs" / current_job).glob("*.json"))
        try:
            _, _, job = find_job(sky, current_job)
            related_divisions = list(job.get("related_divisions", []))
        except SkylineError:
            related_divisions = []
    div = agent.get("primary_division")
    divisions = ([div] if div else []) + related_divisions
    for name in sorted(set(d for d in divisions if d)):
        paths += list((division_dir(sky, name) / "inbox").glob("*.json"))
    return sorted(set(paths), key=lambda p: p.name)


def command_inbox(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        agent_path, agent = load_agent(sky, args.agent)
        payload = coordination_messages_locked(sky, agent, bootstrap=not bool(args.unread), full=bool(getattr(args, "full", False)))
        if args.mark_read:
            save_agent(agent_path, agent)
    emit({"agent_id": args.agent, "coordination": payload, "messages": payload["unread_messages"]})
    return 0


def command_name(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        agent_path, agent = load_agent(sky, args.agent)
        agent["display_name"] = args.name.strip()
        agent["agent_name"] = args.name.strip()
        agent["last_heartbeat"] = iso_now()
        write_intent_from_agent(sky, agent)
        save_agent(agent_path, agent)
    emit({"status": "NAMED", "agent_id": args.agent, "display_name": args.name.strip()})
    return 0


def command_react(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        _, agent = load_agent(sky, args.agent)
        message_id = validate_id(args.message, "message id")
        path = sky / "reactions" / f"{message_id}.json"
        record = json_read(path, {"schema_version": SCHEMA_VERSION, "message_id": message_id, "reactions": {}}) or {}
        reactions = record.setdefault("reactions", {})
        if getattr(args, "remove", False):
            reactions.pop(args.agent, None)
        else:
            reaction = (getattr(args, "reaction", None) or "ACK").upper()
            reactions[args.agent] = {"agent_id": args.agent, "agent_name": agent.get("display_name") or agent.get("agent_name"), "reaction": reaction, "at": iso_now()}
        record["updated_at"] = iso_now()
        json_write(path, record)
    emit(record)
    return 0


def command_test_run_start(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        ensure_layout(sky)
        maintenance = maybe_maintain_locked(sky)
        recovered = list(maintenance.get("recovered_test_runs", [])) if maintenance else recover_stale_test_runs_locked(sky)
        agent_path, agent = load_agent(sky, args.agent)
        if str(agent.get("status") or "").upper() in INACTIVE_AGENT_STATUSES:
            agent["status"] = "ACTIVE"
        agent["last_heartbeat"] = iso_now()
        write_intent_from_agent(sky, agent)
        save_agent(agent_path, agent)

        response = None
        for path in (sky / "test-runs" / "active").glob("*.json"):
            current = json_read(path, {})
            if current.get("status") != "ACTIVE" or str(current.get("resource")) != args.resource:
                continue
            if current.get("operator_agent") == args.agent:
                response = dict(current)
                response["resumed"] = True
                break
            raise SkylineError(f"exclusive resource already active: {args.resource} ({current.get('id')})")

        if response is None:
            run_id = f"tr-{utcnow().strftime('%Y%m%d-%H%M%S')}-{secrets.token_hex(3)}"
            now = iso_now()
            record = {
                "schema_version": SCHEMA_VERSION,
                "id": run_id,
                "resource": args.resource,
                "operator_agent": args.agent,
                "operator_name": agent.get("display_name") or agent.get("agent_name"),
                "purpose": args.purpose,
                "source_revision": getattr(args, "revision", None),
                "config_hash": getattr(args, "config_hash", None),
                "checkpoint": getattr(args, "checkpoint", None),
                "telemetry_path": getattr(args, "telemetry", None),
                "status": "ACTIVE",
                "started_at": now,
                "updated_at": now,
            }
            json_write(sky / "test-runs" / "active" / f"{run_id}.json", record)
            response = dict(record)
        if recovered:
            response["recovered_stale_runs"] = recovered
    emit(response)
    return 0


def command_test_run_finish(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        path = sky / "test-runs" / "active" / f"{validate_id(args.id, "test run id")}.json"
        record = json_read(path, None)
        if not isinstance(record, dict):
            raise SkylineError(f"unknown active test run: {args.id}")
        if record.get("operator_agent") != args.agent:
            raise SkylineError(f"test run {args.id} is owned by {record.get("operator_agent")}")
        try:
            agent_path, agent = load_agent(sky, args.agent)
            agent["last_heartbeat"] = iso_now()
            write_intent_from_agent(sky, agent)
            save_agent(agent_path, agent)
        except SkylineError:
            pass
        now = iso_now()
        record["status"] = str(getattr(args, "status", None) or "COMPLETE").upper()
        record["outcome"] = args.outcome
        record["anomalies"] = getattr(args, "anomaly", None) or []
        if getattr(args, "telemetry", None): record["telemetry_path"] = args.telemetry
        record["updated_at"] = now
        record["finished_at"] = now
        destination = sky / "test-runs" / "history" / path.name
        json_write(destination, record)
        path.unlink(missing_ok=True)
    emit(record)
    return 0


def command_test_run_show(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        recover_stale_test_runs_locked(sky)
        active = active_test_runs_locked(sky)
        if getattr(args, "resource", None): active = [row for row in active if str(row.get("resource")) == args.resource]
        history = [json_read(path, {}) for path in (sky / "test-runs" / "history").glob("*.json")]
        if getattr(args, "resource", None): history = [row for row in history if str(row.get("resource")) == args.resource]
        history.sort(key=lambda row: str(row.get("finished_at") or row.get("updated_at") or ""), reverse=True)
        if not getattr(args, "full", False):
            history = history[: int(load_config(sky).get("recent_test_runs", DEFAULT_RECENT_TEST_RUNS))]
    emit({"active": active, "recent": history})
    return 0


def claim_health_locked(sky: Path) -> dict[str, Any]:
    active: dict[str, dict[str, Any]] = {}
    owners: dict[str, list[str]] = {}
    anomalies: list[dict[str, Any]] = []
    for path in (sky / "jobs" / "active").glob("*.json"):
        job = json_read(path, {})
        job_id = str(job.get("id") or path.stem)
        active[job_id] = job
        agent_id = job.get("assigned_agent")
        if agent_id:
            owners.setdefault(agent_id, []).append(job_id)
        record = json_read(claim_path(sky, job_id), None)
        if not record:
            anomalies.append({"type": "ACTIVE_WITHOUT_CLAIM", "job_id": job_id, "agent_id": agent_id})
            continue
        if record.get("job_id") != job_id or record.get("agent_id") != agent_id or record.get("claim_id") != job.get("claim_id"):
            anomalies.append({"type": "CLAIM_MISMATCH", "job_id": job_id, "agent_id": agent_id})
        if int(record.get("generation", -1)) != int(job.get("claim_generation", -2)):
            anomalies.append({"type": "GENERATION_MISMATCH", "job_id": job_id, "agent_id": agent_id})
        if not agent_id:
            anomalies.append({"type": "ACTIVE_WITHOUT_AGENT", "job_id": job_id})
        else:
            agent_state_path = sky / "agents" / str(agent_id) / "state.json"
            agent_state = json_read(agent_state_path, None)
            if not agent_state:
                anomalies.append({"type": "MISSING_AGENT_STATE", "job_id": job_id, "agent_id": agent_id})
            elif (
                agent_state.get("current_job") != job_id
                or agent_state.get("current_claim_id") != job.get("claim_id")
                or int(agent_state.get("claim_generation", -1) or -1) != int(job.get("claim_generation", -2))
            ):
                anomalies.append({"type": "AGENT_STATE_MISMATCH", "job_id": job_id, "agent_id": agent_id})

    for agent_id, jobs in owners.items():
        if len(jobs) > 1:
            anomalies.append({"type": "AGENT_MULTI_CLAIM", "agent_id": agent_id, "job_ids": sorted(jobs)})

    orphan_claims = []
    for path in (sky / "claims").glob("*.json"):
        record = json_read(path, {})
        job_id = str(record.get("job_id") or path.stem)
        if job_id not in active:
            orphan_claims.append(path)
            anomalies.append({"type": "ORPHAN_CLAIM", "job_id": job_id, "path": str(path)})

    duplicate_state = []
    for job_id in active:
        pending = sky / "jobs" / "pending" / f"{job_id}.json"
        if pending.exists():
            duplicate_state.append(job_id)
            anomalies.append({"type": "ACTIVE_AND_PENDING", "job_id": job_id})

    return {
        "active_claims": len(active),
        "anomalies": anomalies,
        "orphan_claim_paths": [str(p) for p in orphan_claims],
        "duplicate_state_jobs": duplicate_state,
    }


def cleanup_orphan_claims_locked(sky: Path) -> list[str]:
    health = claim_health_locked(sky)
    removed = []
    for raw in health["orphan_claim_paths"]:
        path = Path(raw)
        path.unlink(missing_ok=True)
        removed.append(path.stem)
    return removed


def recover_agent_multi_claims_locked(sky: Path) -> list[str]:
    """Recover duplicate legacy reservations without dictating the worker's model state."""
    grouped: dict[str, list[tuple[Path, dict[str, Any]]]] = {}
    for path in (sky / "jobs" / "active").glob("*.json"):
        job = json_read(path, {})
        agent_id = job.get("assigned_agent")
        if agent_id:
            grouped.setdefault(agent_id, []).append((path, job))

    recovered: list[str] = []
    for agent_id, entries in grouped.items():
        if len(entries) <= 1:
            continue
        agent_path = sky / "agents" / agent_id
        state = json_read(agent_path / "state.json", {}) if agent_path.exists() else {}
        preferred = state.get("current_job")
        keep_ids = {preferred} if preferred and any(j.get("id") == preferred for _, j in entries) else set()
        if not keep_ids and entries:
            keep_ids = {entries[0][1].get("id")}
        for path, job in entries:
            if job.get("id") in keep_ids:
                continue
            old_claim = job.get("claim_id")
            release_claim_locked(sky, job)
            job["status"] = "PENDING"
            job["assigned_agent"] = None
            job["updated_at"] = iso_now()
            notes = list(job.get("progress_notes", []))
            notes.append({"at": iso_now(), "agent": "skyline", "note": f"recovered duplicate advisory claim from {agent_id}; old claim {old_claim}"})
            job["progress_notes"] = notes[-30:]
            move_job(path, sky / "jobs" / "pending" / path.name, job)
            recovered.append(job["id"])
        if agent_path.exists() and recovered:
            state["advisory_claim_warning"] = "duplicate legacy claims were recovered; reassess overlapping mutation if relevant"
            state.setdefault("next_action", "reassess mission and peer intents")
            json_write(agent_path / "state.json", state)
            write_intent_from_agent(sky, state)
    return recovered

def recover_stale_active_locked(sky: Path, stale_after: int) -> list[str]:
    """Expire stale legacy reservations while preserving worker autonomy/state."""
    now = utcnow()
    recovered = []
    for path in list((sky / "jobs" / "active").glob("*.json")):
        job = json_read(path, {})
        agent_id = job.get("assigned_agent")
        claim_id = job.get("claim_id")
        record = json_read(claim_path(sky, str(job.get("id") or path.stem)), None)
        agent_path = sky / "agents" / str(agent_id) if agent_id else None
        state = json_read(agent_path / "state.json", {}) if agent_path and agent_path.exists() else {}
        hb = parse_time(state.get("last_heartbeat"))
        lease = parse_time(record.get("lease_expires_at")) if record else None
        structurally_valid = bool(
            agent_id and claim_id and record and state
            and record.get("agent_id") == agent_id
            and record.get("claim_id") == claim_id
            and int(record.get("generation", -1)) == int(job.get("claim_generation", -2))
        )
        heartbeat_fresh = hb is not None and (now - hb).total_seconds() <= stale_after
        lease_fresh = lease is not None and now <= lease
        if structurally_valid and heartbeat_fresh and lease_fresh:
            continue

        old_claim = claim_id
        release_claim_locked(sky, job)
        job["status"] = "PENDING"
        job["assigned_agent"] = None
        job["updated_at"] = iso_now()
        notes = list(job.get("progress_notes", []))
        reason = "invalid advisory claim" if not structurally_valid else "expired/stale advisory lease"
        notes.append({"at": iso_now(), "agent": "skyline", "note": f"recovered {reason} from {agent_id}; old claim {old_claim}"})
        job["progress_notes"] = notes[-30:]
        move_job(path, sky / "jobs" / "pending" / path.name, job)
        if agent_path and agent_path.exists():
            if state.get("current_claim_id") == old_claim:
                state["current_claim_id"] = None
                state["claim_generation"] = None
                state["current_job"] = None
            state["advisory_claim_warning"] = f"{reason}; reassess only the potentially overlapping mutation"
            state["updated_at"] = iso_now()
            json_write(agent_path / "state.json", state)
            write_intent_from_agent(sky, state)
        recovered.append(job["id"])
    return recovered

def coordination_record_age_seconds(record: dict[str, Any], path: Path, now: datetime) -> float | None:
    for key in ("finished_at", "updated_at", "created_at", "started_at"):
        parsed = parse_time(record.get(key)) if isinstance(record, dict) else None
        if parsed is not None:
            return max(0.0, (now - parsed).total_seconds())
    try:
        return max(0.0, now.timestamp() - path.stat().st_mtime)
    except OSError:
        return None


def archive_coordination_file(sky: Path, path: Path) -> bool:
    if not path.is_file():
        return False
    try:
        relative = path.relative_to(sky)
    except ValueError:
        return False
    target = sky / "archive" / "coordination" / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists():
        target = target.with_name(f"{target.stem}-{utcnow().strftime('%Y%m%dT%H%M%S%f')}-{secrets.token_hex(2)}{target.suffix}")
    shutil.move(str(path), str(target))
    return True


def archive_old_coordination_locked(sky: Path) -> dict[str, int]:
    cfg = load_config(sky)
    now = utcnow()
    routine_retention = int(cfg["coordination_retention_seconds"])
    priority_retention = int(cfg["priority_coordination_retention_seconds"])
    user_relayed_retention = int(cfg["user_relayed_coordination_retention_seconds"])
    moved = {"messages": 0, "outcomes": 0, "intents": 0, "reactions": 0, "test_runs": 0, "whiteboard": 0}

    message_paths: set[Path] = set()
    for pattern in ("agents/*/inbox/*.json", "messages/all/*.json", "messages/jobs/**/*.json", "divisions/*/inbox/*.json"):
        message_paths.update(path for path in sky.glob(pattern) if path.is_file())
    for path in sorted(message_paths):
        record = json_read(path, {}) or {}
        retention = routine_retention
        if bool(record.get("user_relayed")):
            retention = max(retention, user_relayed_retention)
        elif message_needs_attention(record):
            retention = max(retention, priority_retention)
        age = coordination_record_age_seconds(record, path, now)
        if age is not None and age > retention and archive_coordination_file(sky, path):
            moved["messages"] += 1

    for path in list((sky / "reactions").glob("*.json")):
        record = json_read(path, {}) or {}
        age = coordination_record_age_seconds(record, path, now)
        if age is not None and age > routine_retention and archive_coordination_file(sky, path):
            moved["reactions"] += 1

    outcomes = []
    for path in (sky / "outcomes").glob("*.json"):
        record = json_read(path, {}) or {}
        parsed = parse_time(record.get("created_at"))
        stamp = parsed.timestamp() if parsed is not None else path.stat().st_mtime
        outcomes.append((stamp, path, record))
    outcomes.sort(key=lambda item: item[0], reverse=True)
    keep_outcomes = int(cfg.get("recent_outcomes", DEFAULT_RECENT_OUTCOMES))
    for _, path, record in outcomes[keep_outcomes:]:
        age = coordination_record_age_seconds(record, path, now)
        if age is not None and age > routine_retention and archive_coordination_file(sky, path):
            moved["outcomes"] += 1

    live_ids = set(live_agent_ids_locked(sky))
    for path in (sky / "intents").glob("*.json"):
        record = json_read(path, {}) or {}
        if str(record.get("agent_id") or path.stem) in live_ids:
            continue
        age = coordination_record_age_seconds(record, path, now)
        if age is not None and age > routine_retention and archive_coordination_file(sky, path):
            moved["intents"] += 1

    history = []
    for path in (sky / "test-runs" / "history").glob("*.json"):
        record = json_read(path, {}) or {}
        parsed = parse_time(record.get("finished_at") or record.get("updated_at"))
        stamp = parsed.timestamp() if parsed is not None else path.stat().st_mtime
        history.append((stamp, path, record))
    history.sort(key=lambda item: item[0], reverse=True)
    keep_runs = int(cfg.get("recent_test_runs", DEFAULT_RECENT_TEST_RUNS))
    for _, path, record in history[keep_runs:]:
        age = coordination_record_age_seconds(record, path, now)
        if age is not None and age > routine_retention and archive_coordination_file(sky, path):
            moved["test_runs"] += 1

    for path in (sky / "whiteboard" / "cards").glob("*.json"):
        record = json_read(path, {}) or {}
        if str(record.get("status") or "open").lower() in {"open", "active"}:
            continue
        age = coordination_record_age_seconds(record, path, now)
        if age is not None and age > routine_retention and archive_coordination_file(sky, path):
            moved["whiteboard"] += 1
    return moved


def recover_stale_test_runs_locked(sky: Path) -> list[str]:
    cfg = load_config(sky)
    now = utcnow()
    stale_after = int(cfg["test_run_stale_after_seconds"])
    recovered: list[str] = []
    for path in list((sky / "test-runs" / "active").glob("*.json")):
        record = json_read(path, {}) or {}
        if str(record.get("status") or "").upper() != "ACTIVE":
            continue
        owner = str(record.get("operator_agent") or "")
        state = json_read(sky / "agents" / owner / "state.json", {}) if owner else {}
        explicit_inactive = bool(state) and str(state.get("status") or "").upper() in INACTIVE_AGENT_STATUSES
        owner_stale = not state or not agent_is_live(state, cfg, now)
        touched = parse_time(record.get("updated_at") or record.get("started_at"))
        lease_old = touched is None or (now - touched).total_seconds() > stale_after
        if not explicit_inactive and not (owner_stale and lease_old):
            continue
        reason = "owner became inactive" if explicit_inactive else f"owner stale and lease older than {stale_after}s"
        finished = iso_now()
        record["status"] = "ABANDONED"
        record["outcome"] = record.get("outcome") or f"Skyline recovered stale exclusive-resource lease: {reason}"
        record["recovered_by"] = "skyline"
        record["recovery_reason"] = reason
        record["updated_at"] = finished
        record["finished_at"] = finished
        destination = sky / "test-runs" / "history" / path.name
        json_write(destination, record)
        path.unlink(missing_ok=True)
        recovered.append(str(record.get("id") or path.stem))
    return recovered

def archive_done_locked(sky: Path, archive_after: int) -> list[str]:
    now = utcnow()
    archived = []
    index = sky / "jobs" / "archive" / "INDEX.jsonl"
    for path in list((sky / "jobs" / "done").glob("*.json")):
        job = json_read(path, {})
        completed = parse_time(job.get("completed_at") or job.get("updated_at"))
        if completed is None or (now - completed).total_seconds() < archive_after:
            continue
        destination = sky / "jobs" / "archive" / path.name
        move_job(path, destination, job)
        tombstone = {
            "id": job.get("id"),
            "title": job.get("title"),
            "kind": job.get("kind"),
            "division": job.get("primary_division"),
            "status": job.get("status"),
            "completed_at": job.get("completed_at"),
            "result_summary": job.get("result_summary"),
            "archived_at": iso_now(),
        }
        with index.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(tombstone, ensure_ascii=False) + "\n")
        job_messages = sky / "messages" / "jobs" / str(job.get("id"))
        if job_messages.exists():
            archived_messages = sky / "archive" / "messages" / "jobs" / str(job.get("id"))
            archived_messages.parent.mkdir(parents=True, exist_ok=True)
            if archived_messages.exists():
                shutil.rmtree(archived_messages)
            shutil.move(str(job_messages), str(archived_messages))

        completed_by = job.get("completed_by")
        if completed_by:
            workspace = sky / "agents" / str(completed_by) / "workspace" / str(job.get("id"))
            if workspace.exists():
                archived_workspace = sky / "archive" / "agents" / str(completed_by) / "workspace" / str(job.get("id"))
                archived_workspace.parent.mkdir(parents=True, exist_ok=True)
                if archived_workspace.exists():
                    shutil.rmtree(archived_workspace)
                shutil.move(str(workspace), str(archived_workspace))
        archived.append(job.get("id"))
    return archived


def maintain_locked(sky: Path) -> dict[str, Any]:
    cfg = load_config(sky)
    removed_orphans = cleanup_orphan_claims_locked(sky)
    recovered_multi = recover_agent_multi_claims_locked(sky)
    recovered = recover_stale_active_locked(sky, int(cfg["stale_after_seconds"]))
    recovered_test_runs = recover_stale_test_runs_locked(sky)
    archived = archive_done_locked(sky, int(cfg["archive_after_seconds"]))
    archived_coordination = archive_old_coordination_locked(sky)
    health = claim_health_locked(sky)
    result = {
        "removed_orphan_claims": removed_orphans,
        "recovered_multi_claim_jobs": recovered_multi,
        "recovered_jobs": recovered,
        "recovered_test_runs": recovered_test_runs,
        "archived_jobs": archived,
        "archived_coordination": archived_coordination,
        "claim_health": health,
    }
    json_write(sky / ".maintenance.json", {"last_run": iso_now()})
    return result


def maybe_maintain_locked(sky: Path) -> dict[str, Any] | None:
    cfg = load_config(sky)
    marker = json_read(sky / ".maintenance.json", {}) or {}
    last_run = parse_time(marker.get("last_run"))
    if last_run is not None and (utcnow() - last_run).total_seconds() < int(cfg["maintenance_interval_seconds"]):
        return None
    return maintain_locked(sky)


def command_maintain(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        result = maintain_locked(sky)
    if not getattr(args, "quiet", False):
        emit(result)
    return 0


def command_intent(args: argparse.Namespace) -> int:
    """Publish the model's own current state, assessment, intent, and next action."""
    ns = argparse.Namespace(
        root=args.root,
        agent=args.agent,
        claim=getattr(args, "claim", None),
        state=args.state,
        assessment=args.assessment,
        intent=args.intent,
        scope=args.scope,
        decision_basis=args.decision_basis,
        next_action=args.next_action,
        job=args.job,
        clear_job=args.clear_job,
        wait_kind=None,
        wake_condition=None,
        note=args.note,
    )
    return command_sync(ns)


def command_outcome(args: argparse.Namespace) -> int:
    """Record a durable result without forcing a global reassessment cycle."""
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        ensure_layout(sky)
        agent_path, agent = load_agent(sky, args.agent)
        related = getattr(args, "job", None)
        if related:
            find_job(sky, related)
        record = record_outcome_locked(sky, agent, args.summary, related)
        agent["last_heartbeat"] = iso_now()
        write_intent_from_agent(sky, agent)
        save_agent(agent_path, agent)
        summary = team_summary_locked(sky)
    emit({"status": "OUTCOME_RECORDED", "agent_id": args.agent, "outcome": record, "team_summary": summary, "instruction": "Continue, pivot, or stop based on mission value. Recording an outcome does not require another coordination round-trip."})
    return 0


def command_migrate(args: argparse.Namespace) -> int:
    """Upgrade an existing Skyline run to autonomous live-delta coordination without deleting history."""
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        ensure_layout(sky)
        cfg = load_config(sky)
        cfg["schema_version"] = SCHEMA_VERSION
        cfg["coordination_mode"] = "autonomous"
        cfg["claim_mode"] = "legacy_advisory"
        cfg["snapshot_mode"] = "live_delta"
        json_write(config_path(sky), cfg)
        migrated_agents = []
        for path in sorted((sky / "agents").iterdir() if (sky / "agents").exists() else []):
            state_path = path / "state.json"
            if not path.is_dir() or not state_path.exists():
                continue
            state = json_read(state_path, {})
            state["schema_version"] = SCHEMA_VERSION
            state.setdefault("assessment", "")
            state.setdefault("intent", "")
            state.setdefault("intent_scope", [])
            state.setdefault("associated_job", state.get("current_job"))
            state.setdefault("decision_basis", "")
            if state.get("status") in ("WAITING", "BLOCKED", "LOST_CLAIM"):
                hb = parse_time(state.get("last_heartbeat"))
                fresh = hb is not None and (utcnow() - hb).total_seconds() <= int(cfg["stale_after_seconds"])
                state["legacy_status"] = state.get("status")
                state["status"] = "REASSESSING" if fresh else "STALE_LEGACY"
            if not state.get("next_action") or str(state.get("next_action", "")).startswith("wait for a runnable"):
                state["next_action"] = "choose and perform the highest-value useful action; coordinate only on meaningful overlap" if state.get("status") != "STALE_LEGACY" else "refresh this chat before treating it as live"
            save_agent(path, state)
            write_intent_from_agent(sky, state)
            migrated_agents.append(state.get("agent_id"))
        runtime = deploy_runtime(sky)
    emit({
        "status": "MIGRATED",
        "skyline": str(sky),
        "runtime": str(runtime),
        "schema_version": SCHEMA_VERSION,
        "coordination_mode": "autonomous",
        "claim_mode": "legacy_advisory",
        "snapshot_mode": "live_delta",
        "migrated_agents": migrated_agents,
        "note": "Historical jobs/claims were preserved. Claims are now compatibility/advisory state; worker intent is primary.",
    })
    return 0


def command_status(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        ensure_layout(sky)
        cfg = load_config(sky)
        maintenance = maybe_maintain_locked(sky)
        jobs: dict[str, list[dict[str, Any]]] = {}
        job_omitted: dict[str, int] = {}
        if args.full:
            for state, directory in job_locations(sky):
                if state == "archive" and not args.include_archive:
                    continue
                rows = [json_read(path, {}) for path in directory.glob("*.json")]
                rows.sort(key=lambda row: (int(row.get("priority", 100)), str(row.get("updated_at") or row.get("created_at") or ""), str(row.get("id") or "")))
                jobs[state] = rows
        else:
            opportunities = opportunity_snapshot_locked(sky)
            jobs = {state: list(rows) for state, rows in dict(opportunities.get("by_state", {})).items()}
            counts = dict(opportunities.get("counts", {}))
            for state in CURRENT_OPPORTUNITY_STATES:
                omitted = int(counts.get(state, 0)) - len(jobs.get(state, []))
                if omitted > 0:
                    job_omitted[state] = omitted

        if args.full:
            agents = []
            for path in (sky / "agents").iterdir() if (sky / "agents").exists() else []:
                if path.is_dir() and (path / "state.json").exists():
                    state = json_read(path / "state.json", {})
                    if agent_is_live(state, cfg):
                        agents.append(compact_agent_state(state))
        else:
            live_states, _ = live_agent_states_locked(sky)
            agents = [compact_agent_state(state) for state in live_states]
        agents.sort(key=lambda row: str(row.get("updated_at") or ""), reverse=True)
        divisions = sorted(path.name for path in (sky / "divisions").iterdir() if path.is_dir())
        outcomes = recent_outcomes_locked(sky)
        test_history = [json_read(path, {}) for path in (sky / "test-runs" / "history").glob("*.json")]
        test_history.sort(key=lambda row: str(row.get("finished_at") or row.get("updated_at") or ""), reverse=True)
        test_history = test_history[: int(cfg.get("recent_test_runs", DEFAULT_RECENT_TEST_RUNS))]
        payload = {
            "maintenance": maintenance or {"implicit": True, "ran": False},
            "config": cfg,
            "mission": str(sky / "SKYLINE.md"),
            "team_summary": team_summary_locked(sky),
            "divisions": divisions,
            "jobs_as_opportunities": jobs,
            "agents": agents,
            "recent_outcomes": outcomes,
            "test_runs": {"active": active_test_runs_locked(sky), "recent": test_history},
        }
        if job_omitted:
            payload["jobs_omitted"] = job_omitted
        if args.full:
            payload["intents"] = [json_read(intent_path(sky, str(agent.get("agent_id"))), {}) for agent in agents]
    emit(payload)
    return 0

def command_gc(args: argparse.Namespace) -> int:
    sky = skyline_dir(args.root)
    require_initialized(sky)
    with locked(sky):
        cfg = load_config(sky)
        if args.archive_after is not None:
            cfg["archive_after_seconds"] = args.archive_after
            json_write(config_path(sky), cfg)
        result = maintain_locked(sky)
        if args.prune_tmp:
            pruned = []
            for agent in (sky / "agents").iterdir():
                tmp = agent / "tmp"
                if not tmp.is_dir():
                    continue
                for child in list(tmp.iterdir()):
                    if child.is_dir():
                        shutil.rmtree(child, ignore_errors=True)
                    else:
                        child.unlink(missing_ok=True)
                pruned.append(agent.name)
            result["pruned_tmp_agents"] = pruned
    emit(result)
    return 0


def add_root(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--root", default=".", help="directory containing skyline/, or skyline/ itself")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("init", help="create or repair the shared Skyline coordination folder")
    add_root(p)
    p.add_argument("--title")
    p.add_argument("--source-root", help="canonical source workspace represented by this Skyline project")
    p.add_argument("--runtime-digest", help="digest of the runtime that provisioned this Skyline project")
    p.add_argument("--archive-after", type=int, help="seconds before completed jobs leave active context")
    p.add_argument("--stale-after", type=int, help="seconds before an abandoned active job is recoverable")
    p.add_argument("--max-active-agents", type=positive_int, help="legacy claim compatibility ceiling; not a worker-count policy")
    p.set_defaults(func=command_init)

    p = sub.add_parser("division-add", help="create/update a soft context division")
    add_root(p)
    p.add_argument("--name", required=True)
    p.add_argument("--context")
    p.set_defaults(func=command_division_add)

    p = sub.add_parser("job-add", help="add a seeded opportunity/responsibility record (not a worker assignment)")
    add_root(p)
    p.add_argument("--id")
    p.add_argument("--title", required=True)
    p.add_argument("--instructions", required=True)
    p.add_argument("--kind", choices=("WORK", "VERIFY", "INTEGRATE", "RESEARCH", "FINALIZE"), default="WORK")
    p.add_argument("--role-hint", default="generalist")
    p.add_argument("--division", default="general")
    p.add_argument("--related", action="append")
    p.add_argument("--priority", type=int, default=100, help="lower numbers surface earlier as opportunity hints")
    p.add_argument("--depends-on", action="append")
    p.add_argument("--validation", choices=("none", "required"), default="none")
    p.add_argument("--ownership", action="append")
    p.add_argument("--created-by", default="constructor")
    p.add_argument("--target-job")
    p.set_defaults(func=command_job_add)

    p = sub.add_parser("start", help="load/resume a worker and return mission/team state for model-driven decision making")
    p.add_argument("--start", default=".", help="starting path used to search this directory and its ancestors")
    p.add_argument("--agent", help="existing agent id for this chat; omit on first activation")
    p.add_argument("--full", action="store_true")
    p.set_defaults(func=command_start)

    p = sub.add_parser("seat", help="compatibility alias for start; load mission/team state without auto-assignment")
    p.add_argument("--start", default=".", help="starting path used to search this directory and its ancestors")
    p.add_argument("--agent", help="existing agent id for this chat; omit on first activation")
    p.add_argument("--full", action="store_true")
    p.set_defaults(func=command_seat)

    p = sub.add_parser("next", help="refresh mission/team state so the worker can decide its own next action")
    add_root(p)
    p.add_argument("--agent", help="existing agent id; omit on first activation")
    p.add_argument("--job", help="optionally associate this opportunity with the worker while it reassesses")
    p.add_argument("--full", action="store_true")
    p.set_defaults(func=command_next)

    p = sub.add_parser("guard", help="report legacy/advisory claim health; not a work authorization check")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", required=True)
    p.add_argument("--renew", action="store_true", help="also renew the claim lease")
    p.set_defaults(func=command_guard)

    p = sub.add_parser("sync", help="persist model-authored intent/state, read messages, and show peer/team state")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", help="optional legacy advisory claim id")
    p.add_argument("--state", help="model-authored current state, e.g. ACTIVE, REASSESSING, VALIDATING")
    p.add_argument("--assessment")
    p.add_argument("--intent")
    p.add_argument("--scope", action="append")
    p.add_argument("--decision-basis")
    p.add_argument("--job", help="optional associated opportunity; does not constrain the worker")
    p.add_argument("--clear-job", action="store_true")
    p.add_argument("--next-action")
    p.add_argument("--note")
    p.add_argument("--full", action="store_true")
    p.set_defaults(func=command_sync)

    p = sub.add_parser("update", help="compatibility alias for updating model-authored worker state")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", help="optional legacy advisory claim id")
    p.add_argument("--status", help="free-form concise model-authored state")
    p.add_argument("--assessment")
    p.add_argument("--intent")
    p.add_argument("--scope", action="append")
    p.add_argument("--decision-basis")
    p.add_argument("--job")
    p.add_argument("--clear-job", action="store_true")
    p.add_argument("--next-action")
    p.add_argument("--wait-kind")
    p.add_argument("--wake-condition")
    p.add_argument("--note")
    p.set_defaults(func=command_update)

    p = sub.add_parser("finish", help="record a durable outcome; with a valid legacy claim, optionally complete that compatibility job")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", help="optional legacy advisory claim id")
    p.add_argument("--job", help="optional related opportunity for an unclaimed outcome")
    p.add_argument("--result", required=True, help="evidence-based result summary; also used as legacy VERIFY summary")
    p.add_argument("--validation-result", choices=("PASS", "FAIL", "INCONCLUSIVE"), help="required only for VERIFY jobs")
    p.set_defaults(func=command_finish)

    p = sub.add_parser("complete", help="finish the current job and optionally request independent validation")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", required=True)
    p.add_argument("--result", required=True)
    p.add_argument("--needs-validation", action="store_true")
    p.set_defaults(func=command_complete)

    p = sub.add_parser("validate", help="finish the current VERIFY job and update its target")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", required=True)
    p.add_argument("--result", required=True, choices=("PASS", "FAIL", "INCONCLUSIVE"))
    p.add_argument("--summary", required=True)
    p.set_defaults(func=command_validate)

    p = sub.add_parser("yield", help="record a blocked dependency and reassess; optionally release a legacy advisory claim")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", help="optional legacy advisory claim id")
    p.add_argument("--reason", required=True)
    p.add_argument("--wake-condition", required=True)
    p.add_argument("--kind", default="DEPENDENCY")
    p.add_argument("--requeue", action="store_true", help="leave job pending; without this it is blocked")
    p.set_defaults(func=command_yield)

    p = sub.add_parser("pivot", help="leave a handoff and reassess; never auto-allocate the next job")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", help="optional legacy advisory claim id")
    p.add_argument("--note", required=True, help="durable handoff/progress note for the job being released")
    p.add_argument("--to-job", help="optionally associate a new opportunity after the handoff; does not claim it")
    p.set_defaults(func=command_pivot)

    p = sub.add_parser("wake", help="clear a pending job's wait marker")
    add_root(p)
    p.add_argument("--job", required=True)
    p.set_defaults(func=command_wake)

    p = sub.add_parser("send", help="send an immutable agent/job/division/broadcast message")
    add_root(p)
    p.add_argument("--agent", required=True, help="sender agent id")
    p.add_argument("--to", action="append", required=True, help="agent:<id>, job:<id>, division:<name>, all, or bare agent id")
    p.add_argument("--type", choices=("INFO", "QUESTION", "RESPONSE", "ALERT", "BLOCKER", "DECISION_PROPOSAL", "REVIEW_REQUEST"), default="INFO")
    p.add_argument("--subject", required=True)
    p.add_argument("--body", required=True)
    p.add_argument("--related", action="append")
    p.add_argument("--requires-response", action="store_true")
    p.add_argument("--user-relayed", action="store_true")
    p.set_defaults(func=command_send)

    p = sub.add_parser("name", help="set a concise worker display name")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--name", required=True)
    p.set_defaults(func=command_name)

    p = sub.add_parser("react", help="record or remove a lightweight message reaction")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--message", required=True)
    p.add_argument("--reaction")
    p.add_argument("--remove", action="store_true")
    p.set_defaults(func=command_react)

    p = sub.add_parser("test-run", help="coordinate an exclusive shared test resource")
    test_sub = p.add_subparsers(dest="test_run_command", required=True)
    q = test_sub.add_parser("start")
    add_root(q)
    q.add_argument("--agent", required=True)
    q.add_argument("--resource", required=True)
    q.add_argument("--purpose", required=True)
    q.add_argument("--revision")
    q.add_argument("--config-hash")
    q.add_argument("--checkpoint")
    q.add_argument("--telemetry")
    q.set_defaults(func=command_test_run_start)
    q = test_sub.add_parser("finish")
    add_root(q)
    q.add_argument("--agent", required=True)
    q.add_argument("--id", required=True)
    q.add_argument("--outcome", required=True)
    q.add_argument("--status")
    q.add_argument("--anomaly", action="append")
    q.add_argument("--telemetry")
    q.set_defaults(func=command_test_run_finish)
    q = test_sub.add_parser("show")
    add_root(q)
    q.add_argument("--resource")
    q.add_argument("--full", action="store_true")
    q.set_defaults(func=command_test_run_show)

    p = sub.add_parser("inbox", help="read relevant messages for an agent")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--unread", action="store_true")
    p.add_argument("--mark-read", action="store_true")
    p.add_argument("--full", action="store_true")
    p.set_defaults(func=command_inbox)

    p = sub.add_parser("intent", help="publish the model's current state, assessment, intent, scope, and next action")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--claim", help="optional legacy advisory claim id")
    p.add_argument("--state", default="ACTIVE")
    p.add_argument("--assessment", default="")
    p.add_argument("--intent", required=True)
    p.add_argument("--scope", action="append")
    p.add_argument("--decision-basis")
    p.add_argument("--next-action", required=True)
    p.add_argument("--job", help="optional associated opportunity")
    p.add_argument("--clear-job", action="store_true")
    p.add_argument("--note")
    p.set_defaults(func=command_intent)

    p = sub.add_parser("outcome", help="record a durable worker-authored result without forcing reassessment")
    add_root(p)
    p.add_argument("--agent", required=True)
    p.add_argument("--summary", required=True)
    p.add_argument("--job", help="optional related opportunity")
    p.set_defaults(func=command_outcome)

    p = sub.add_parser("migrate", help="upgrade an existing run to autonomous live-delta coordination")
    add_root(p)
    p.set_defaults(func=command_migrate)

    p = sub.add_parser("status", help="show mission-driven agents, intents, opportunities, outcomes, and compatibility state")
    add_root(p)
    p.add_argument("--include-archive", action="store_true")
    p.add_argument("--full", action="store_true")
    p.set_defaults(func=command_status)

    p = sub.add_parser("maintain", help="recover stale work and archive old completed jobs")
    add_root(p)
    p.set_defaults(func=command_maintain)

    p = sub.add_parser("gc", help="run lifecycle cleanup; optionally prune per-agent tmp folders")
    add_root(p)
    p.add_argument("--archive-after", type=int)
    p.add_argument("--prune-tmp", action="store_true")
    p.set_defaults(func=command_gc)

    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        return int(args.func(args) or 0)
    except (SkylineError, FileNotFoundError, json.JSONDecodeError) as exc:
        print(f"skyline: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
