#include "lib_classify.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if defined(_WIN32)
#include <windows.h>
#else
#include <dlfcn.h>
#endif

typedef int (*corvid_abi_verify_fn)(const uint8_t expected[32]);
typedef CorvidCallStatus (*corvid_call_agent_fn)(
    const char* agent_name,
    const char* args_json,
    size_t args_len,
    char** out_result,
    size_t* out_result_len,
    uint64_t* out_observation_handle,
    CorvidApprovalRequired* out_approval);
typedef void (*corvid_free_result_fn)(char* result);
typedef void (*corvid_observation_release_fn)(uint64_t handle);
typedef CorvidHostEventStatus (*corvid_record_host_event_fn)(
    const char* name,
    const char* payload_json,
    size_t payload_len);

static int decode_hex_64(const char* hex, uint8_t out[32]) {
    size_t len = strlen(hex);
    if (len != 64) {
        return 0;
    }
    for (size_t i = 0; i < 32; ++i) {
        char hi = hex[i * 2];
        char lo = hex[i * 2 + 1];
        uint8_t hi_val;
        uint8_t lo_val;
        if (hi >= '0' && hi <= '9') {
            hi_val = (uint8_t)(hi - '0');
        } else if (hi >= 'a' && hi <= 'f') {
            hi_val = (uint8_t)(hi - 'a' + 10);
        } else if (hi >= 'A' && hi <= 'F') {
            hi_val = (uint8_t)(hi - 'A' + 10);
        } else {
            return 0;
        }
        if (lo >= '0' && lo <= '9') {
            lo_val = (uint8_t)(lo - '0');
        } else if (lo >= 'a' && lo <= 'f') {
            lo_val = (uint8_t)(lo - 'a' + 10);
        } else if (lo >= 'A' && lo <= 'F') {
            lo_val = (uint8_t)(lo - 'A' + 10);
        } else {
            return 0;
        }
        out[i] = (uint8_t)((hi_val << 4) | lo_val);
    }
    return 1;
}

static void set_demo_env(const char* trace_path) {
#if defined(_WIN32)
    DeleteFileA(trace_path);
#else
    remove(trace_path);
#endif
#if defined(_WIN32)
    _putenv_s("CORVID_MODEL", "mock-1");
    _putenv_s("CORVID_TEST_MOCK_LLM", "1");
    _putenv_s("CORVID_TEST_MOCK_LLM_REPLIES", "{\"classify_prompt\":\"positive\"}");
    _putenv_s("CORVID_TRACE_PATH", trace_path);
    _putenv_s("CORVID_TRACE_DISABLE", "");
    _putenv_s("CORVID_REPLAY_TRACE_PATH", "");
#else
    setenv("CORVID_MODEL", "mock-1", 1);
    setenv("CORVID_TEST_MOCK_LLM", "1", 1);
    setenv("CORVID_TEST_MOCK_LLM_REPLIES", "{\"classify_prompt\":\"positive\"}", 1);
    setenv("CORVID_TRACE_PATH", trace_path, 1);
    unsetenv("CORVID_TRACE_DISABLE");
    unsetenv("CORVID_REPLAY_TRACE_PATH");
#endif
}

#if defined(_WIN32)
static FARPROC load_symbol(HMODULE library, const char* name) {
    return GetProcAddress(library, name);
}
#else
static void* load_symbol(void* library, const char* name) {
    return dlsym(library, name);
}
#endif

int main(int argc, char** argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: %s <library> <expected_hash_hex> <trace_path>\n", argv[0]);
        return 2;
    }

    uint8_t expected_hash[32];
    if (!decode_hex_64(argv[2], expected_hash)) {
        fprintf(stderr, "expected hash must be 64 hex chars\n");
        return 2;
    }

    set_demo_env(argv[3]);

#if defined(_WIN32)
    HMODULE library = LoadLibraryA(argv[1]);
    if (library == NULL) {
        fprintf(stderr, "failed to load %s\n", argv[1]);
        return 1;
    }
#else
    void* library = dlopen(argv[1], RTLD_NOW);
    if (library == NULL) {
        fprintf(stderr, "failed to load %s: %s\n", argv[1], dlerror());
        return 1;
    }
#endif

    corvid_abi_verify_fn corvid_abi_verify =
        (corvid_abi_verify_fn)load_symbol(library, "corvid_abi_verify");
    corvid_call_agent_fn corvid_call_agent =
        (corvid_call_agent_fn)load_symbol(library, "corvid_call_agent");
    corvid_free_result_fn corvid_free_result =
        (corvid_free_result_fn)load_symbol(library, "corvid_free_result");
    corvid_observation_release_fn corvid_observation_release =
        (corvid_observation_release_fn)load_symbol(library, "corvid_observation_release");
    corvid_record_host_event_fn corvid_record_host_event =
        (corvid_record_host_event_fn)load_symbol(library, "corvid_record_host_event");

    if (!corvid_abi_verify || !corvid_call_agent || !corvid_free_result ||
        !corvid_observation_release || !corvid_record_host_event) {
        fprintf(stderr, "required capsule symbol missing\n");
        return 1;
    }

    printf("verified=%d\n", corvid_abi_verify(expected_hash));

    {
        const char* payload = "{\"host\":\"c\",\"event\":\"capsule_record\"}";
        CorvidHostEventStatus status =
            corvid_record_host_event("capsule_record", payload, strlen(payload));
        printf("host_event_status=%u\n", (unsigned)status);
    }

    {
        const char* bad_payload = "{";
        CorvidHostEventStatus status =
            corvid_record_host_event("bad_json", bad_payload, strlen(bad_payload));
        printf("bad_json_status=%u\n", (unsigned)status);
    }

    {
        const char* args_json = "[\"I loved the support experience\"]";
        char* result = NULL;
        size_t result_len = 0;
        uint64_t observation_handle = CORVID_NULL_OBSERVATION_HANDLE;
        CorvidApprovalRequired approval = {0};
        CorvidCallStatus status = corvid_call_agent(
            "classify",
            args_json,
            strlen(args_json),
            &result,
            &result_len,
            &observation_handle,
            &approval);
        printf("replay_line=status=%u result=%.*s\n", (unsigned)status, (int)result_len, result);
        corvid_observation_release(observation_handle);
        corvid_free_result(result);
    }

    printf("trace_path=%s\n", argv[3]);

#if defined(_WIN32)
    FreeLibrary(library);
#else
    dlclose(library);
#endif
    return 0;
}
