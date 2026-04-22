#include "lib_classify.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if defined(_WIN32)
#include <windows.h>
#define PATH_SEP "\\"
#else
#include <dlfcn.h>
#define PATH_SEP "/"
#endif

typedef const char* (*corvid_abi_descriptor_json_fn)(size_t* out_len);
typedef void (*corvid_abi_descriptor_hash_fn)(uint8_t out_hash[32]);
typedef int (*corvid_abi_verify_fn)(const uint8_t expected[32]);
typedef size_t (*corvid_list_agents_fn)(CorvidAgentHandle* out, size_t capacity);
typedef CorvidFindAgentsResult (*corvid_find_agents_where_fn)(
    const char* filter_json,
    size_t filter_len,
    size_t* out_indices,
    size_t out_cap);
typedef CorvidPreFlight (*corvid_pre_flight_fn)(const char* agent_name, const char* args_json, size_t args_len);
typedef CorvidCallStatus (*corvid_call_agent_fn)(
    const char* agent_name,
    const char* args_json,
    size_t args_len,
    char** out_result,
    size_t* out_result_len,
    uint64_t* out_observation_handle,
    CorvidApprovalRequired* out_approval);
typedef void (*corvid_free_result_fn)(char* result);
typedef int32_t (*corvid_grounded_sources_fn)(uint64_t handle, const char** out, size_t capacity);
typedef double (*corvid_grounded_confidence_fn)(uint64_t handle);
typedef void (*corvid_grounded_release_fn)(uint64_t handle);
typedef double (*corvid_observation_cost_usd_fn)(uint64_t handle);
typedef uint64_t (*corvid_observation_latency_ms_fn)(uint64_t handle);
typedef uint64_t (*corvid_observation_tokens_in_fn)(uint64_t handle);
typedef uint64_t (*corvid_observation_tokens_out_fn)(uint64_t handle);
typedef bool (*corvid_observation_exceeded_bound_fn)(uint64_t handle);
typedef void (*corvid_observation_release_fn)(uint64_t handle);

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

static void set_demo_env(void) {
#if defined(_WIN32)
    _putenv_s("CORVID_MODEL", "mock-1");
    _putenv_s("CORVID_TEST_MOCK_LLM", "1");
    _putenv_s("CORVID_TEST_MOCK_LLM_REPLIES", "{\"classify_prompt\":\"positive\"}");
#else
    setenv("CORVID_MODEL", "mock-1", 1);
    setenv("CORVID_TEST_MOCK_LLM", "1", 1);
    setenv("CORVID_TEST_MOCK_LLM_REPLIES", "{\"classify_prompt\":\"positive\"}", 1);
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
    const char* filter_arg = NULL;
    if (argc != 3 && argc != 4) {
        fprintf(stderr, "usage: %s <library> <expected_hash_hex> [--filter=<json>]\n", argv[0]);
        return 2;
    }
    if (argc == 4) {
        if (strncmp(argv[3], "--filter=", 9) != 0) {
            fprintf(stderr, "expected optional argument in the form --filter=<json>\n");
            return 2;
        }
        filter_arg = argv[3] + 9;
    }

    set_demo_env();

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
    corvid_list_agents_fn corvid_list_agents =
        (corvid_list_agents_fn)load_symbol(library, "corvid_list_agents");
    corvid_find_agents_where_fn corvid_find_agents_where =
        (corvid_find_agents_where_fn)load_symbol(library, "corvid_find_agents_where");
    corvid_pre_flight_fn corvid_pre_flight =
        (corvid_pre_flight_fn)load_symbol(library, "corvid_pre_flight");
    corvid_call_agent_fn corvid_call_agent =
        (corvid_call_agent_fn)load_symbol(library, "corvid_call_agent");
    corvid_free_result_fn corvid_free_result =
        (corvid_free_result_fn)load_symbol(library, "corvid_free_result");
    corvid_grounded_sources_fn corvid_grounded_sources =
        (corvid_grounded_sources_fn)load_symbol(library, "corvid_grounded_sources");
    corvid_grounded_confidence_fn corvid_grounded_confidence =
        (corvid_grounded_confidence_fn)load_symbol(library, "corvid_grounded_confidence");
    corvid_grounded_release_fn corvid_grounded_release =
        (corvid_grounded_release_fn)load_symbol(library, "corvid_grounded_release");
    corvid_observation_cost_usd_fn corvid_observation_cost_usd =
        (corvid_observation_cost_usd_fn)load_symbol(library, "corvid_observation_cost_usd");
    corvid_observation_latency_ms_fn corvid_observation_latency_ms =
        (corvid_observation_latency_ms_fn)load_symbol(library, "corvid_observation_latency_ms");
    corvid_observation_tokens_in_fn corvid_observation_tokens_in =
        (corvid_observation_tokens_in_fn)load_symbol(library, "corvid_observation_tokens_in");
    corvid_observation_tokens_out_fn corvid_observation_tokens_out =
        (corvid_observation_tokens_out_fn)load_symbol(library, "corvid_observation_tokens_out");
    corvid_observation_exceeded_bound_fn corvid_observation_exceeded_bound =
        (corvid_observation_exceeded_bound_fn)load_symbol(library, "corvid_observation_exceeded_bound");
    corvid_observation_release_fn corvid_observation_release =
        (corvid_observation_release_fn)load_symbol(library, "corvid_observation_release");
    const char* (*grounded_tag_fn)(const char*, uint64_t*) =
        (const char* (*)(const char*, uint64_t*))load_symbol(library, "grounded_tag");
    void (*corvid_free_string_fn)(const char*) =
        (void (*)(const char*))load_symbol(library, "corvid_free_string");

    if (!corvid_abi_verify || !corvid_list_agents || !corvid_find_agents_where || !corvid_pre_flight ||
        !corvid_call_agent || !corvid_free_result || !corvid_grounded_sources ||
        !corvid_grounded_confidence || !corvid_grounded_release || !corvid_observation_cost_usd ||
        !corvid_observation_latency_ms || !corvid_observation_tokens_in || !corvid_observation_tokens_out ||
        !corvid_observation_exceeded_bound || !corvid_observation_release || !grounded_tag_fn ||
        !corvid_free_string_fn) {
        fprintf(stderr, "required catalog symbol missing\n");
        return 1;
    }

    uint8_t expected_hash[32];
    if (!decode_hex_64(argv[2], expected_hash)) {
        fprintf(stderr, "expected hash must be 64 hex chars\n");
        return 2;
    }

    {
        int verified = corvid_abi_verify(expected_hash);
        printf("verified=%d\n", verified);
    }

    {
        size_t count = corvid_list_agents(NULL, 0);
        CorvidAgentHandle* handles = (CorvidAgentHandle*)calloc(count, sizeof(CorvidAgentHandle));
        if (handles == NULL) {
            fprintf(stderr, "out of memory\n");
            return 1;
        }
        corvid_list_agents(handles, count);
        printf("agent_count=%zu\n", count);
        if (count > 0) {
            printf("first_agent=%s\n", handles[0].name);
        }
        free(handles);
    }

    if (filter_arg != NULL) {
        size_t count = corvid_list_agents(NULL, 0);
        size_t* indices = (size_t*)calloc(count == 0 ? 1 : count, sizeof(size_t));
        CorvidFindAgentsResult filtered = corvid_find_agents_where(
            filter_arg,
            strlen(filter_arg),
            indices,
            count);
        printf("filter_status=%u filtered_count=%zu\n", (unsigned)filtered.status, filtered.matched_count);
        if (filtered.status == CORVID_FIND_AGENTS_OK && filtered.matched_count > 0) {
            CorvidAgentHandle* handles = (CorvidAgentHandle*)calloc(count, sizeof(CorvidAgentHandle));
            if (handles == NULL) {
                fprintf(stderr, "out of memory\n");
                free(indices);
                return 1;
            }
            corvid_list_agents(handles, count);
            for (size_t i = 0; i < filtered.matched_count && i < count; ++i) {
                size_t index = indices[i];
                if (index < count) {
                    printf("filtered_agent=%s\n", handles[index].name);
                }
            }
            free(handles);
        } else if (filtered.error_message != NULL) {
            printf("filter_error=%s\n", filtered.error_message);
        }
        free(indices);
    }

    {
        const char* args_json = "[\"I loved the support experience\"]";
        CorvidPreFlight preflight = corvid_pre_flight("classify", args_json, strlen(args_json));
        printf(
            "preflight_status=%u cost_bound_usd=%.2f requires_approval=%u\n",
            (unsigned)preflight.status,
            preflight.cost_bound_usd,
            (unsigned)preflight.requires_approval);
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
        printf("call_status=%u result=%.*s\n", (unsigned)status, (int)result_len, result);
        printf(
            "observation_handle=%llu cost_usd=%.4f latency_ms=%llu tokens_in=%llu tokens_out=%llu exceeded_bound=%u\n",
            (unsigned long long)observation_handle,
            corvid_observation_cost_usd(observation_handle),
            (unsigned long long)corvid_observation_latency_ms(observation_handle),
            (unsigned long long)corvid_observation_tokens_in(observation_handle),
            (unsigned long long)corvid_observation_tokens_out(observation_handle),
            (unsigned)corvid_observation_exceeded_bound(observation_handle));
        corvid_observation_release(observation_handle);
        corvid_free_result(result);
    }

    {
        uint64_t grounded_handle = CORVID_NULL_GROUNDED_HANDLE;
        const char* grounded_value = grounded_tag_fn("catalog-proof", &grounded_handle);
        const char* sources[4] = {0};
        int32_t source_count = corvid_grounded_sources(grounded_handle, sources, 4);
        double grounded_conf = corvid_grounded_confidence(grounded_handle);
        printf(
            "grounded_result=%s grounded_handle=%llu grounded_source_count=%d grounded_confidence=%.2f\n",
            grounded_value,
            (unsigned long long)grounded_handle,
            (int)source_count,
            grounded_conf);
        if (source_count > 0 && sources[0] != NULL) {
            printf("grounded_source=%s\n", sources[0]);
        }
        corvid_grounded_release(grounded_handle);
        corvid_free_string_fn(grounded_value);
    }

#if defined(_WIN32)
    FreeLibrary(library);
#else
    dlclose(library);
#endif
    return 0;
}
