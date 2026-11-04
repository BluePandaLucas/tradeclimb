#!/usr/bin/env python3
"""TradeClimb agent client: standard library only, Unix socket or authenticated HTTP."""

import argparse, datetime, hashlib, http.client, json, os, pathlib, socket, stat, sys, time, urllib.parse, urllib.request, urllib.error

TERMINAL = {"completed", "failed", "cancelled", "interrupted"}


class ApiError(RuntimeError):
    def __init__(self, status, message):
        self.status = status
        self.code = f"http_{status}"
        super().__init__(message)


class Client:
    def __init__(self, socket_path=None, url=None, token_file=None):
        self.socket = socket_path or os.environ.get("TC_API_SOCKET")
        self.url = url or os.environ.get("TC_API_URL", "http://127.0.0.1:8080")
        path = token_file or os.environ.get("TC_API_TOKEN_FILE")
        if not path:
            raise ValueError(
                "TC_API_TOKEN_FILE must identify a scoped agent credential"
            )
        self.key = pathlib.Path(path).read_text().strip()

    def request(self, path, body=None):
        payload = (
            None if body is None else json.dumps(body, ensure_ascii=False).encode()
        )
        headers = {
            "Authorization": "Bearer " + self.key,
            "Content-Type": "application/json",
        }
        retry = (
            body is None
            or bool(body.get("idempotency_key"))
            or path.endswith("/cancel")
        )
        for attempt in range(3):
            connection = None
            try:
                if self.socket:
                    connection = http.client.HTTPConnection("localhost", timeout=30)
                    connection.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    connection.sock.settimeout(30)
                    connection.sock.connect(self.socket)
                    connection.request(
                        "GET" if body is None else "POST",
                        path,
                        body=payload,
                        headers=headers,
                    )
                    response = connection.getresponse()
                    status = response.status
                    raw = response.read(8 * 1024 * 1024 + 1)
                else:
                    req = urllib.request.Request(
                        self.url.rstrip("/") + path, data=payload, headers=headers
                    )
                    try:
                        response = urllib.request.urlopen(req, timeout=30)
                    except urllib.error.HTTPError as error:
                        response = error
                    with response:
                        status = response.status
                        raw = response.read(8 * 1024 * 1024 + 1)
                if len(raw) > 8 * 1024 * 1024:
                    raise ValueError("Response exceeded 8 MiB")
                try:
                    result = json.loads(raw)
                except (ValueError, UnicodeDecodeError):
                    result = {"error": raw.decode(errors="replace")[:2000]}
                if status >= 400:
                    if status >= 500 and retry and attempt < 2:
                        time.sleep(0.2 * 2**attempt)
                        continue
                    raise ApiError(status, result.get("error", str(result)))
                return result
            except (OSError, http.client.HTTPException):
                if not retry or attempt == 2:
                    raise
                time.sleep(0.2 * 2**attempt)
            finally:
                if connection:
                    connection.close()

    def wait(self, kind, identifier, seconds=2000):
        path = (
            "/api/agent/candidates/" if kind == "candidate" else "/api/jobs/"
        ) + identifier
        deadline = time.monotonic() + seconds
        while True:
            value = self.request(path)
            if value["state"] in TERMINAL:
                return value
            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"Wait expired; resume with wait-{kind} {identifier}"
                )
            time.sleep(0.5)

    def initialize(self, directory):
        kit = self.request("/api/agent/kit")
        root = pathlib.Path(directory)
        root.mkdir(mode=0o700, parents=True, exist_ok=False)
        for name, body in kit["files"].items():
            path = pathlib.PurePosixPath(name)
            if path.is_absolute() or ".." in path.parts:
                raise ValueError("Invalid kit path")
            target = root.joinpath(*path.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(body)
        return {
            "project": str(root),
            "platform": kit["platform"],
            "source": "src/main.rs",
            "guide": "AGENT.md",
        }

    def submit(
        self,
        project,
        name,
        description="",
        parent=None,
        parameters=None,
        markets=None,
        fixtures=None,
        key=None,
    ):
        root = pathlib.Path(project)
        platform = json.loads((root / ".tc-agent.json").read_text())["platform"]
        body = {
            "platform": platform,
            "name": name,
            "description": description,
            "parent": parent,
            "parameters": parameters or {},
            "markets": markets or ["BTC-EUR"],
            "files": read_source(root),
            "model_fixtures": fixtures or {},
        }
        body["idempotency_key"] = (
            key
            or "source-"
            + hashlib.sha256(
                json.dumps(body, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest()
        )
        result = self.request("/api/agent/candidates", body)
        (root / ".tc-agent-last.json").write_text(
            json.dumps(
                {"candidate": result["id"], "fingerprint": result["fingerprint"]},
                indent=2,
            )
            + "\n"
        )
        return result

    def run(self, strategy, campaign, key=None, trace=None, slippage_bps=None):
        body = {
            "strategy": strategy,
            "campaign": campaign,
            "period": "development",
            "trace": trace,
            "slippage_bps": slippage_bps,
        }
        body["idempotency_key"] = (
            key
            or "development-"
            + hashlib.sha256(
                json.dumps(body, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest()
        )
        return self.request("/api/jobs", body)


def read_source(root):
    # Pin every directory component with no-follow descriptors; never copy symlinks,
    # FIFOs or concurrently swapped parent paths when a human uses the client locally.
    absolute = pathlib.Path(os.path.abspath(root))
    fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for name in (*absolute.parts[1:], "src"):
            child = os.open(
                name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd
            )
            os.close(fd)
            fd = child
        files = {}
        size = 0

        def visit(directory, prefix, depth=0):
            nonlocal size
            if depth > 16:
                raise ValueError("Source nesting exceeds 16 directories")
            for name in sorted(os.listdir(directory)):
                meta = os.stat(name, dir_fd=directory, follow_symlinks=False)
                if stat.S_ISLNK(meta.st_mode):
                    raise ValueError("Source symlinks are not supported")
                path = prefix + "/" + name
                if stat.S_ISDIR(meta.st_mode):
                    child = os.open(
                        name,
                        os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                        dir_fd=directory,
                    )
                    try:
                        visit(child, path, depth + 1)
                    finally:
                        os.close(child)
                elif name.endswith(".rs"):
                    child = os.open(
                        name,
                        os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                        dir_fd=directory,
                    )
                    try:
                        if not stat.S_ISREG(os.fstat(child).st_mode):
                            raise ValueError("Regular Rust files required")
                        with os.fdopen(child, "rb", closefd=False) as source:
                            raw = source.read(65537)
                    finally:
                        os.close(child)
                    if len(raw) > 65536:
                        raise ValueError("Rust file exceeds 64 KiB")
                    size += len(raw)
                    files[path] = raw.decode()
                    if size > 262144 or len(files) > 64:
                        raise ValueError("Source bundle exceeds limits")
                else:
                    raise ValueError("Only Rust source files are supported under src/")

        visit(fd, "src")
        return files
    finally:
        os.close(fd)


def timestamp(value):
    try:
        return int(value)
    except ValueError:
        date = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
        if date.tzinfo is None:
            date = date.replace(tzinfo=datetime.timezone.utc)
        return int(date.timestamp() * 1000)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ["capabilities", "overview"]:
        commands.add_parser(name)
    p = commands.add_parser("init")
    p.add_argument("directory")
    p = commands.add_parser("submit")
    p.add_argument("project")
    p.add_argument("--name", required=True)
    p.add_argument("--description", default="")
    p.add_argument("--parent")
    p.add_argument("--parameters", default="{}")
    p.add_argument("--markets", nargs="+", default=["BTC-EUR"])
    p.add_argument("--model-fixtures", default="{}")
    p.add_argument("--key")
    p.add_argument("--wait", action="store_true")
    for name in [
        "candidate",
        "wait-candidate",
        "cancel-candidate",
        "job",
        "wait-job",
        "cancel-job",
        "diagnostics",
        "trace",
    ]:
        p = commands.add_parser(name)
        p.add_argument("id")
    p = commands.add_parser("run")
    p.add_argument("--strategy", required=True)
    p.add_argument("--campaign", required=True)
    p.add_argument("--key")
    p.add_argument("--trace-start")
    p.add_argument("--trace-end")
    p.add_argument("--trace-frames", type=int, default=256)
    p.add_argument("--slippage-bps", type=int)
    p.add_argument("--wait", action="store_true")
    p = commands.add_parser("candles")
    p.add_argument("--campaign", required=True)
    p.add_argument("--market", required=True)
    p.add_argument("--start", required=True)
    p.add_argument("--end", required=True)
    p.add_argument("--limit", type=int, default=1000)
    args = parser.parse_args()
    client = Client()
    command = args.command
    if command == "capabilities":
        result = client.request("/api/agent/capabilities")
    elif command == "overview":
        result = client.request("/api/overview")
    elif command == "init":
        result = client.initialize(args.directory)
    elif command == "submit":
        result = client.submit(
            args.project,
            args.name,
            args.description,
            args.parent,
            json.loads(args.parameters),
            args.markets,
            json.loads(args.model_fixtures),
            args.key,
        )
        if args.wait:
            result = client.wait("candidate", result["id"])
    elif command == "run":
        if bool(args.trace_start) != bool(args.trace_end):
            raise ValueError("Supply both --trace-start and --trace-end")
        trace = (
            None
            if not args.trace_start
            else {
                "start": timestamp(args.trace_start),
                "end": timestamp(args.trace_end),
                "max_frames": args.trace_frames,
            }
        )
        result = client.run(
            args.strategy, args.campaign, args.key, trace, args.slippage_bps
        )
        if args.wait:
            result = client.wait("job", result["id"])
    elif command == "candles":
        result = client.request(
            "/api/agent/candles?"
            + urllib.parse.urlencode(
                {
                    "campaign": args.campaign,
                    "market": args.market,
                    "start": timestamp(args.start),
                    "end": timestamp(args.end),
                    "limit": args.limit,
                }
            )
        )
    elif command == "candidate":
        result = client.request("/api/agent/candidates/" + args.id)
    elif command.startswith("wait-"):
        result = client.wait(command[5:], args.id)
    elif command == "cancel-candidate":
        result = client.request("/api/agent/candidates/" + args.id + "/cancel", {})
    elif command == "job":
        result = client.request("/api/jobs/" + args.id)
    elif command == "cancel-job":
        result = client.request("/api/jobs/" + args.id + "/cancel", {})
    else:
        result = client.request("/api/agent/jobs/" + args.id + "/" + command)
    print(json.dumps(result, indent=2, ensure_ascii=True))
    if isinstance(result, dict) and result.get("state") in {"failed", "interrupted"}:
        return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ApiError, ValueError, OSError, TimeoutError, KeyError) as error:
        print(
            json.dumps(
                {"error": str(error), "code": getattr(error, "code", "client_error")}
            ),
            file=sys.stderr,
        )
        sys.exit(2)
