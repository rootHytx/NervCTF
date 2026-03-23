"""
NervCTF Instance Challenge Plugin for CTFd.

Provides the "instance" challenge type which proxies instance lifecycle
management to the NervCTF remote-monitor service.

Required environment variables (set in CTFd's .env):
  NERVCTF_MONITOR_URL    — URL of the remote-monitor (e.g. http://10.0.0.1:33133)
  NERVCTF_MONITOR_TOKEN  — Admin token for the remote-monitor
"""

import logging
import os
from datetime import datetime, timezone

import requests as req_lib
from CTFd.models import (
    ChallengeFiles,
    Challenges,
    Fails,
    Flags,
    Hints,
    Solves,
    Tags,
    db,
)
from CTFd.plugins import register_plugin_assets_directory
from CTFd.plugins.challenges import CHALLENGE_CLASSES, BaseChallenge
from CTFd.utils.decorators import authed_only
from CTFd.utils.user import get_current_team
from flask import Blueprint, request

logger = logging.getLogger(__name__)


# ── Helpers ───────────────────────────────────────────────────────────────────

def _monitor():
    """Return {"url": str, "token": str} or None if not configured."""
    url = os.environ.get("NERVCTF_MONITOR_URL", "").rstrip("/")
    token = os.environ.get("NERVCTF_MONITOR_TOKEN", "")
    if url and token:
        return {"url": url, "token": token}
    return None


def _monitor_headers(m):
    return {"Authorization": f"Token {m['token']}", "Content-Type": "application/json"}


def _monitor_json(resp):
    """Parse JSON from a monitor response, raising ValueError with context on failure."""
    ct = resp.headers.get("content-type", "")
    if "application/json" not in ct:
        preview = resp.text[:300] if resp.text else "(empty)"
        raise ValueError(f"Monitor returned HTTP {resp.status_code} non-JSON ({ct!r}): {preview}")
    return resp.json()


def _team_id():
    team = get_current_team()
    return team.id if team else None


def _sqlite_to_ms(s):
    """Convert 'YYYY-MM-DD HH:MM:SS' (UTC) to Unix milliseconds."""
    try:
        dt = datetime.strptime(s, "%Y-%m-%d %H:%M:%S").replace(tzinfo=timezone.utc)
        return int(dt.timestamp() * 1000)
    except Exception:
        return 0


def _to_connection(inst):
    """Convert monitor instance row dict to Docker-plugin-compatible connection object."""
    return {
        "type": inst.get("connection_type", "nc"),
        "host": inst.get("host", ""),
        "port": inst.get("port", 0),
    }


# ── Challenge type ─────────────────────────────────────────────────────────────

class InstanceChallengeType(BaseChallenge):
    id = "instance"
    name = "instance"
    templates = {
        "create": "/plugins/nervctf_instance/assets/create.html",
        "update": "/plugins/nervctf_instance/assets/update.html",
        "view": "/plugins/nervctf_instance/assets/view.html",
    }
    scripts = {
        "create": "/plugins/nervctf_instance/assets/create.js",
        "update": "/plugins/nervctf_instance/assets/update.js",
        "view": "/plugins/nervctf_instance/assets/view.js",
    }

    @classmethod
    def create(cls, request):
        from .models.challenge import InstanceChallenge

        data = request.form or request.get_json() or {}

        challenge = InstanceChallenge(
            name=data.get("name", ""),
            description=data.get("description", ""),
            category=data.get("category", ""),
            value=int(data.get("value") or 0),
            state=data.get("state", "hidden"),
            max_attempts=int(data.get("max_attempts") or 0),
            type="instance",
            backend=data.get("backend", "docker"),
            image=data.get("image", ""),
            command=data.get("command", ""),
            compose_file=data.get("compose_file", "docker-compose.yml"),
            compose_service=data.get("compose_service", ""),
            lxc_image=data.get("lxc_image", ""),
            vagrantfile=data.get("vagrantfile", ""),
            internal_port=int(data.get("internal_port") or 1337),
            connection=data.get("connection", "nc"),
            timeout_minutes=int(data.get("timeout_minutes") or 45),
            max_renewals=int(data.get("max_renewals") or 3),
            flag_mode=data.get("flag_mode", "static"),
            flag_prefix=data.get("flag_prefix", ""),
            flag_suffix=data.get("flag_suffix", ""),
            random_flag_length=int(data.get("random_flag_length") or 16),
        )

        # Dynamic scoring fields
        if data.get("initial"):
            challenge.initial_value = int(data["initial"])
            challenge.minimum_value = int(data.get("minimum") or 0)
            challenge.decay_value = int(data.get("decay") or 0)
            challenge.decay_function = data.get("decay_function", "linear")
            challenge.value = challenge.initial_value

        db.session.add(challenge)
        db.session.commit()

        _register_with_monitor(challenge)
        return challenge

    @classmethod
    def read(cls, challenge):
        from .models.challenge import InstanceChallenge

        chall = InstanceChallenge.query.filter_by(id=challenge.id).first()
        m = _monitor()

        data = {
            "id": challenge.id,
            "name": challenge.name,
            "value": challenge.value,
            "description": challenge.description,
            "category": challenge.category,
            "state": challenge.state,
            "max_attempts": challenge.max_attempts,
            "type": challenge.type,
            "type_data": {
                "id": cls.id,
                "name": cls.name,
                "templates": cls.templates,
                "scripts": cls.scripts,
            },
        }

        if chall:
            data.update(
                {
                    "backend": chall.backend,
                    "connection": chall.connection,
                    "timeout_minutes": chall.timeout_minutes,
                    "max_renewals": chall.max_renewals,
                    "flag_mode": chall.flag_mode,
                    "monitor_url": m["url"] if m else None,
                }
            )

        return data

    @classmethod
    def update(cls, challenge, request):
        from .models.challenge import InstanceChallenge

        data = request.form or request.get_json() or {}

        challenge.name = data.get("name", challenge.name)
        challenge.description = data.get("description", challenge.description)
        challenge.category = data.get("category", challenge.category)
        challenge.state = data.get("state", challenge.state)
        challenge.max_attempts = int(data.get("max_attempts") or 0)

        if data.get("value"):
            challenge.value = int(data["value"])

        chall = InstanceChallenge.query.filter_by(id=challenge.id).first()
        if chall:
            chall.backend = data.get("backend", chall.backend)
            chall.image = data.get("image", chall.image)
            chall.command = data.get("command", chall.command)
            chall.compose_file = data.get("compose_file", chall.compose_file)
            chall.compose_service = data.get("compose_service", chall.compose_service)
            chall.lxc_image = data.get("lxc_image", chall.lxc_image)
            chall.vagrantfile = data.get("vagrantfile", chall.vagrantfile)
            chall.internal_port = int(data.get("internal_port") or chall.internal_port)
            chall.connection = data.get("connection", chall.connection)
            chall.timeout_minutes = int(data.get("timeout_minutes") or chall.timeout_minutes)
            chall.max_renewals = int(data.get("max_renewals") or chall.max_renewals)
            chall.flag_mode = data.get("flag_mode", chall.flag_mode)
            chall.flag_prefix = data.get("flag_prefix", chall.flag_prefix)
            chall.flag_suffix = data.get("flag_suffix", chall.flag_suffix)
            chall.random_flag_length = int(
                data.get("random_flag_length") or chall.random_flag_length
            )
            if data.get("initial"):
                chall.initial_value = int(data["initial"])
                chall.minimum_value = int(data.get("minimum") or 0)
                chall.decay_value = int(data.get("decay") or 0)
                chall.decay_function = data.get("decay_function", "linear")
                challenge.value = chall.initial_value

        db.session.commit()
        _register_with_monitor(chall or challenge)
        return challenge

    @classmethod
    def delete(cls, challenge):
        _stop_all_instances(challenge)

        Fails.query.filter_by(challenge_id=challenge.id).delete()
        Solves.query.filter_by(challenge_id=challenge.id).delete()
        Flags.query.filter_by(challenge_id=challenge.id).delete()
        Hints.query.filter_by(challenge_id=challenge.id).delete()
        Tags.query.filter_by(challenge_id=challenge.id).delete()
        ChallengeFiles.query.filter_by(challenge_id=challenge.id).delete()

        from .models.challenge import InstanceChallenge

        InstanceChallenge.query.filter_by(id=challenge.id).delete()
        Challenges.query.filter_by(id=challenge.id).delete()
        db.session.commit()

    @classmethod
    def attempt(cls, challenge, request):
        result = BaseChallenge.attempt(challenge, request)
        # CTFd returns a tuple (bool, str) in older versions and a ChallengeResponse
        # object in newer versions. Handle both without assuming attribute names.
        try:
            is_correct = bool(result[0])
        except TypeError:
            is_correct = bool(getattr(result, "success", False))
        json_data = request.get_json(silent=True) or {}
        submitted = (json_data.get("submission") or request.form.get("submission") or "").strip()
        team_id = _team_id()
        from CTFd.utils.user import get_current_user
        user = get_current_user()
        user_id = user.id if user else None
        if team_id and user_id and submitted:
            m = _monitor()
            if m:
                try:
                    resp = req_lib.post(
                        f"{m['url']}/api/v1/plugin/attempt",
                        headers=_monitor_headers(m),
                        json={
                            "challenge_name": challenge.name,
                            "team_id": team_id,
                            "user_id": user_id,
                            "submitted_flag": submitted,
                            "is_correct": is_correct,
                        },
                        timeout=2,
                    )
                    if not resp.ok:
                        logger.warning("Monitor attempt POST failed: %s %s", resp.status_code, resp.text[:200])
                except Exception as e:
                    logger.warning("Monitor attempt POST error: %s", e)
        return result

    @classmethod
    def solve(cls, user, team, challenge, request):
        BaseChallenge.solve(user, team, challenge, request)
        # Tear down the instance now that the team has solved the challenge.
        team_id = team.id if team else None
        if team_id:
            m = _monitor()
            if m:
                json_data = request.get_json(silent=True) or {}
                submitted = (json_data.get("submission") or request.form.get("submission") or "").strip()
                try:
                    req_lib.post(
                        f"{m['url']}/api/v1/plugin/solve",
                        headers=_monitor_headers(m),
                        json={
                            "challenge_name": challenge.name,
                            "team_id": team_id,
                            "user_id": user.id if user else None,
                            "submitted_flag": submitted,
                        },
                        timeout=10,
                    )
                except Exception as e:
                    logger.error("Monitor solve teardown error: %s", e)

    @classmethod
    def fail(cls, user, team, challenge, request):
        BaseChallenge.fail(user, team, challenge, request)


# ── Monitor helpers ───────────────────────────────────────────────────────────

def _register_with_monitor(challenge):
    m = _monitor()
    if not m:
        return
    import json

    config = {
        "backend": getattr(challenge, "backend", "docker"),
        "image": getattr(challenge, "image", None) or None,
        "command": getattr(challenge, "command", None) or None,
        "compose_file": getattr(challenge, "compose_file", None) or None,
        "compose_service": getattr(challenge, "compose_service", None) or None,
        "lxc_image": getattr(challenge, "lxc_image", None) or None,
        "vagrantfile": getattr(challenge, "vagrantfile", None) or None,
        "internal_port": getattr(challenge, "internal_port", 1337),
        "connection": getattr(challenge, "connection", "nc"),
        "timeout_minutes": getattr(challenge, "timeout_minutes", 45),
        "max_renewals": getattr(challenge, "max_renewals", 3),
        "flag_mode": getattr(challenge, "flag_mode", "static"),
        "flag_prefix": getattr(challenge, "flag_prefix", None) or None,
        "flag_suffix": getattr(challenge, "flag_suffix", None) or None,
        "random_flag_length": getattr(challenge, "random_flag_length", 16),
    }
    try:
        resp = req_lib.post(
            f"{m['url']}/api/v1/instance/register",
            headers=_monitor_headers(m),
            json={
                "challenge_name": challenge.name,
                "ctfd_id": challenge.id,
                "backend": config["backend"],
                "config_json": json.dumps(config),
            },
            timeout=5,
        )
        if not resp.ok:
            logger.error("Monitor register failed: %s", resp.text)
    except Exception as e:
        logger.error("Monitor register error: %s", e)


def _stop_all_instances(challenge):
    m = _monitor()
    if not m:
        return
    try:
        req_lib.delete(
            f"{m['url']}/api/v1/plugin/stop_all",
            headers=_monitor_headers(m),
            json={"challenge_name": challenge.name},
            timeout=5,
        )
    except Exception:
        pass


# ── Blueprint — player-facing routes ─────────────────────────────────────────

bp = Blueprint("nervctf_instance", __name__)


@bp.route("/api/v1/containers/info/<int:challenge_id>", methods=["GET"])
@authed_only
def get_instance_info(challenge_id):
    from .models.challenge import InstanceChallenge

    challenge = InstanceChallenge.query.filter_by(id=challenge_id).first()
    if not challenge:
        return {"status": "not_found"}, 200

    team_id = _team_id()
    if not team_id:
        return {"error": "Not in a team"}, 403

    # If the team already solved this challenge, no instance is needed.
    from CTFd.models import Solves
    if Solves.query.filter_by(challenge_id=challenge.id, team_id=team_id).first():
        return {"status": "solved"}, 200

    m = _monitor()
    if not m:
        return {"status": "not_found", "error": "Monitor not configured"}, 200

    try:
        resp = req_lib.get(
            f"{m['url']}/api/v1/plugin/info",
            headers={"Authorization": f"Token {m['token']}"},
            params={"challenge_name": challenge.name, "team_id": team_id},
            timeout=10,
            allow_redirects=False,
        )
        data = _monitor_json(resp)
        if data.get("status") == "running":
            return {
                "status": "running",
                "expires_at": _sqlite_to_ms(data.get("expires_at", "")),
                "connection": _to_connection(data),
            }, 200
        return data, resp.status_code
    except Exception as e:
        return {"error": str(e)}, 500


@bp.route("/api/v1/containers/request", methods=["POST"])
@authed_only
def request_instance():
    from .models.challenge import InstanceChallenge

    body = request.get_json() or {}
    challenge_id = body.get("challenge_id")
    challenge = InstanceChallenge.query.filter_by(id=challenge_id).first()
    if not challenge:
        return {"error": "Challenge not found"}, 404

    team_id = _team_id()
    if not team_id:
        return {"error": "Not in a team"}, 403

    m = _monitor()
    if not m:
        return {"error": "Monitor not configured"}, 503

    from CTFd.utils.user import get_current_user
    user = get_current_user()
    try:
        resp = req_lib.post(
            f"{m['url']}/api/v1/plugin/request",
            headers=_monitor_headers(m),
            json={
                "challenge_name": challenge.name,
                "team_id": team_id,
                "user_id": user.id if user else None,
            },
            timeout=60,
            allow_redirects=False,
        )
        data = _monitor_json(resp)
        if resp.ok and not data.get("error"):
            return {
                "expires_at": _sqlite_to_ms(data.get("expires_at", "")),
                "connection": _to_connection(data),
            }, 200
        return data, resp.status_code
    except Exception as e:
        return {"error": str(e)}, 500


@bp.route("/api/v1/containers/renew", methods=["POST"])
@authed_only
def renew_instance():
    from .models.challenge import InstanceChallenge

    body = request.get_json() or {}
    challenge_id = body.get("challenge_id")
    challenge = InstanceChallenge.query.filter_by(id=challenge_id).first()
    if not challenge:
        return {"error": "Challenge not found"}, 404

    team_id = _team_id()
    if not team_id:
        return {"error": "Not in a team"}, 403

    m = _monitor()
    if not m:
        return {"error": "Monitor not configured"}, 503

    try:
        resp = req_lib.post(
            f"{m['url']}/api/v1/plugin/renew",
            headers=_monitor_headers(m),
            json={"challenge_name": challenge.name, "team_id": team_id},
            timeout=10,
            allow_redirects=False,
        )
        data = _monitor_json(resp)
        if resp.ok and not data.get("error"):
            return {
                "expires_at": _sqlite_to_ms(data.get("expires_at", "")),
                "connection": _to_connection(data),
            }, 200
        return data, resp.status_code
    except Exception as e:
        return {"error": str(e)}, 500


@bp.route("/api/v1/containers/stop", methods=["POST"])
@authed_only
def stop_instance():
    from .models.challenge import InstanceChallenge

    body = request.get_json() or {}
    challenge_id = body.get("challenge_id")
    challenge = InstanceChallenge.query.filter_by(id=challenge_id).first()
    if not challenge:
        return {"error": "Challenge not found"}, 404

    team_id = _team_id()
    if not team_id:
        return {"error": "Not in a team"}, 403

    m = _monitor()
    if not m:
        return {"error": "Monitor not configured"}, 503

    try:
        resp = req_lib.delete(
            f"{m['url']}/api/v1/plugin/stop",
            headers=_monitor_headers(m),
            json={"challenge_name": challenge.name, "team_id": team_id},
            timeout=30,
            allow_redirects=False,
        )
        data = _monitor_json(resp)
        return data, resp.status_code
    except Exception as e:
        return {"error": str(e)}, 500


# ── Plugin entry point ────────────────────────────────────────────────────────

def load(app):
    from CTFd.models import db as _db
    from .models.challenge import InstanceChallenge  # noqa: F401 — registers model

    with app.app_context():
        _db.create_all()

    CHALLENGE_CLASSES["instance"] = InstanceChallengeType
    register_plugin_assets_directory(
        app, base_path="/plugins/nervctf_instance/assets/"
    )
    app.register_blueprint(bp)
    logger.info("NervCTF instance challenge plugin loaded.")
