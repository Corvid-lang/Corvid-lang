import ctypes
import json
import os
import sys


class CorvidApprovalRequired(ctypes.Structure):
    _fields_ = [
        ("site_name", ctypes.c_char_p),
        ("predicate_json", ctypes.c_char_p),
        ("args_json", ctypes.c_char_p),
        ("rationale_prompt", ctypes.c_char_p),
    ]


def deterministic_seed(events):
    for event in events:
        if event.get("kind") == "seed_read" and event.get("purpose") == "rollout_default_seed":
            return int(event["value"])
    for event in events:
        if event.get("kind") == "schema_header":
            return int(event.get("ts_ms", 0))
    return 0


def first_run_started(events):
    for event in events:
        if event.get("kind") == "run_started":
            return event["agent"], event.get("args", [])
    raise RuntimeError("trace had no run_started event")


def recorded_model(events):
    for event in reversed(events):
        if event.get("kind") in ("llm_call", "llm_result"):
            model = event.get("model")
            if model:
                return str(model)
    return None


def load_events(path):
    events = []
    with open(path, "r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                events.append(json.loads(line))
    return events


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <library> <trace>", file=sys.stderr)
        return 2

    events = load_events(sys.argv[2])
    agent, args = first_run_started(events)
    os.environ["CORVID_REPLAY_TRACE_PATH"] = sys.argv[2]
    os.environ["CORVID_TRACE_DISABLE"] = "1"
    os.environ["CORVID_DETERMINISTIC_SEED"] = str(deterministic_seed(events))
    model = recorded_model(events)
    if model:
        os.environ["CORVID_MODEL"] = model

    library = ctypes.CDLL(sys.argv[1])
    library.corvid_call_agent.argtypes = [
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_char_p),
        ctypes.POINTER(ctypes.c_size_t),
        ctypes.POINTER(ctypes.c_uint64),
        ctypes.POINTER(CorvidApprovalRequired),
    ]
    library.corvid_call_agent.restype = ctypes.c_uint32
    library.corvid_free_result.argtypes = [ctypes.c_void_p]
    library.corvid_free_result.restype = None
    library.corvid_observation_release.argtypes = [ctypes.c_uint64]
    library.corvid_observation_release.restype = None

    args_json = json.dumps(args).encode("utf-8")
    result = ctypes.c_char_p()
    result_len = ctypes.c_size_t()
    observation_handle = ctypes.c_uint64()
    approval = CorvidApprovalRequired()
    status = library.corvid_call_agent(
        agent.encode("utf-8"),
        args_json,
        len(args_json),
        ctypes.byref(result),
        ctypes.byref(result_len),
        ctypes.byref(observation_handle),
        ctypes.byref(approval),
    )
    result_text = ctypes.string_at(result, result_len.value).decode("utf-8") if result.value else "null"
    print(f"replay_line=status={status} result={result_text}")
    if observation_handle.value:
        library.corvid_observation_release(observation_handle)
    if result.value:
        library.corvid_free_result(result)
    return 0 if status == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
