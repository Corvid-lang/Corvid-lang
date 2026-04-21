import ctypes
import json
import os
import sys


class CorvidAgentHandle(ctypes.Structure):
    _fields_ = [
        ("name", ctypes.c_char_p),
        ("symbol", ctypes.c_char_p),
        ("source_file", ctypes.c_char_p),
        ("source_line", ctypes.c_uint32),
        ("trust_tier", ctypes.c_uint8),
        ("cost_bound_usd", ctypes.c_double),
        ("reversible", ctypes.c_uint8),
        ("latency_instant", ctypes.c_uint8),
        ("replayable", ctypes.c_uint8),
        ("deterministic", ctypes.c_uint8),
        ("dangerous", ctypes.c_uint8),
        ("pub_extern_c", ctypes.c_uint8),
        ("requires_approval", ctypes.c_uint8),
        ("grounded_source_count", ctypes.c_uint32),
        ("param_count", ctypes.c_uint32),
    ]


class CorvidPreFlight(ctypes.Structure):
    _fields_ = [
        ("status", ctypes.c_uint32),
        ("cost_bound_usd", ctypes.c_double),
        ("requires_approval", ctypes.c_uint8),
        ("effect_row_json", ctypes.c_char_p),
        ("grounded_source_set_json", ctypes.c_char_p),
        ("bad_args_message", ctypes.c_char_p),
    ]


class CorvidApprovalRequired(ctypes.Structure):
    _fields_ = [
        ("site_name", ctypes.c_char_p),
        ("predicate_json", ctypes.c_char_p),
        ("args_json", ctypes.c_char_p),
        ("rationale_prompt", ctypes.c_char_p),
    ]


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <library> <expected-hash-hex>", file=sys.stderr)
        return 2

    os.environ["CORVID_MODEL"] = "mock-1"
    os.environ["CORVID_TEST_MOCK_LLM"] = "1"
    os.environ["CORVID_TEST_MOCK_LLM_REPLIES"] = json.dumps({"classify_prompt": "positive"})

    library = ctypes.CDLL(sys.argv[1])

    library.corvid_abi_verify.argtypes = [ctypes.POINTER(ctypes.c_uint8)]
    library.corvid_abi_verify.restype = ctypes.c_int
    library.corvid_list_agents.argtypes = [ctypes.POINTER(CorvidAgentHandle), ctypes.c_size_t]
    library.corvid_list_agents.restype = ctypes.c_size_t
    library.corvid_pre_flight.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_size_t]
    library.corvid_pre_flight.restype = CorvidPreFlight
    library.corvid_call_agent.argtypes = [
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_char_p),
        ctypes.POINTER(ctypes.c_size_t),
        ctypes.POINTER(CorvidApprovalRequired),
    ]
    library.corvid_call_agent.restype = ctypes.c_uint32
    library.corvid_free_result.argtypes = [ctypes.c_char_p]
    library.corvid_free_result.restype = None

    expected = bytes.fromhex(sys.argv[2])
    expected_buf = (ctypes.c_uint8 * len(expected)).from_buffer_copy(expected)
    print(f"verified={library.corvid_abi_verify(expected_buf)}")

    count = library.corvid_list_agents(None, 0)
    handles = (CorvidAgentHandle * count)()
    library.corvid_list_agents(handles, count)
    print(f"agent_count={count}")
    if count:
        print(f"first_agent={handles[0].name.decode()}")

    args_json = b"[\"I loved the support experience\"]"
    preflight = library.corvid_pre_flight(b"classify", args_json, len(args_json))
    print(
        f"preflight_status={preflight.status} "
        f"cost_bound_usd={preflight.cost_bound_usd:.2f} "
        f"requires_approval={preflight.requires_approval}"
    )

    result = ctypes.c_char_p()
    result_len = ctypes.c_size_t()
    approval = CorvidApprovalRequired()
    status = library.corvid_call_agent(
        b"classify",
        args_json,
        len(args_json),
        ctypes.byref(result),
        ctypes.byref(result_len),
        ctypes.byref(approval),
    )
    print(f"call_status={status} result={ctypes.string_at(result, result_len.value).decode()}")
    library.corvid_free_result(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
