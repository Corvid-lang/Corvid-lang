#include "lib_classify.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if defined(_WIN32)
#include <windows.h>
#else
#include <dlfcn.h>
#include <unistd.h>
#endif

typedef int (*corvid_abi_verify_fn)(const uint8_t expected[32]);
typedef size_t (*corvid_list_agents_fn)(CorvidAgentHandle* out, size_t capacity);
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
typedef void (*corvid_observation_release_fn)(uint64_t handle);
typedef CorvidApproverLoadStatus (*corvid_register_approver_from_source_fn)(
    const char* source_path,
    double max_budget_usd_per_call,
    char** out_error_message);
typedef void (*corvid_clear_approver_fn)(void);

#if defined(_WIN32)
static FARPROC load_symbol(HMODULE library, const char* name) {
    return GetProcAddress(library, name);
}
#else
static void* load_symbol(void* library, const char* name) {
    return dlsym(library, name);
}
#endif

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

static void set_env_var(const char* key, const char* value) {
#if defined(_WIN32)
    _putenv_s(key, value);
#else
    setenv(key, value, 1);
#endif
}

static void clear_env_var(const char* key) {
#if defined(_WIN32)
    _putenv_s(key, "");
#else
    unsetenv(key);
#endif
}

static void set_demo_env(void) {
    set_env_var("CORVID_MODEL", "mock-1");
    set_env_var("CORVID_TEST_MOCK_LLM", "1");
    set_env_var(
        "CORVID_TEST_MOCK_LLM_REPLIES",
        "{\"classify_prompt\":\"positive\"}");
}

static int write_text_file(const char* path, const char* text) {
    FILE* file = fopen(path, "wb");
    if (file == NULL) {
        return 0;
    }
    if (fwrite(text, 1, strlen(text), file) != strlen(text)) {
        fclose(file);
        return 0;
    }
    fclose(file);
    return 1;
}

static int path_parent(const char* input, char* out, size_t out_cap) {
    size_t len = strlen(input);
    size_t i = len;
    while (i > 0) {
        char ch = input[i - 1];
        if (ch == '/' || ch == '\\') {
            if (i > out_cap) {
                return 0;
            }
            memcpy(out, input, i - 1);
            out[i - 1] = '\0';
            return 1;
        }
        i--;
    }
    if (out_cap < 2) {
        return 0;
    }
    out[0] = '.';
    out[1] = '\0';
    return 1;
}

static void join_path(char* out, size_t out_cap, const char* dir, const char* file) {
#if defined(_WIN32)
    const char sep = '\\';
#else
    const char sep = '/';
#endif
    snprintf(out, out_cap, "%s%c%s", dir, sep, file);
}

static int has_agent_named(const CorvidAgentHandle* handles, size_t count, const char* needle) {
    size_t i;
    for (i = 0; i < count; ++i) {
        if (handles[i].name != NULL && strcmp(handles[i].name, needle) == 0) {
            return 1;
        }
    }
    return 0;
}

static int run_issue_tag_call(
    corvid_call_agent_fn corvid_call_agent,
    corvid_free_result_fn corvid_free_result,
    corvid_observation_release_fn corvid_observation_release,
    const char* args_json,
    const char* label,
    CorvidApprovalRequired* out_approval) {
    char* result = NULL;
    size_t result_len = 0;
    uint64_t observation_handle = CORVID_NULL_OBSERVATION_HANDLE;
    CorvidCallStatus status;
    status = corvid_call_agent(
        "issue_tag",
        args_json,
        strlen(args_json),
        &result,
        &result_len,
        &observation_handle,
        out_approval);
    if (status == CORVID_CALL_OK && result != NULL) {
        printf(
            "%s_status=%u result=%.*s observation_handle=%llu\n",
            label,
            (unsigned)status,
            (int)result_len,
            result,
            (unsigned long long)observation_handle);
    } else if (out_approval != NULL && out_approval->site_name != NULL) {
        printf(
            "%s_status=%u site=%s observation_handle=%llu\n",
            label,
            (unsigned)status,
            out_approval->site_name,
            (unsigned long long)observation_handle);
    } else {
        printf(
            "%s_status=%u observation_handle=%llu\n",
            label,
            (unsigned)status,
            (unsigned long long)observation_handle);
    }
    corvid_observation_release(observation_handle);
    corvid_free_result(result);
    return (int)status;
}

int main(int argc, char** argv) {
#if defined(_WIN32)
    HMODULE library;
#else
    void* library;
#endif
    corvid_abi_verify_fn corvid_abi_verify;
    corvid_list_agents_fn corvid_list_agents;
    corvid_pre_flight_fn corvid_pre_flight;
    corvid_call_agent_fn corvid_call_agent;
    corvid_free_result_fn corvid_free_result;
    corvid_observation_release_fn corvid_observation_release;
    corvid_register_approver_from_source_fn corvid_register_approver_from_source;
    corvid_clear_approver_fn corvid_clear_approver;
    uint8_t expected_hash[32];
    size_t agent_count;
    CorvidAgentHandle* agents;
    char approver_dir[1024];
    char reject_approver_path[1200];
    const char* trace_path = "trace_output/approval_demo.jsonl";
    CorvidPreFlight preflight;
    CorvidApprovalRequired approval = {0};
    char* error_message = NULL;

    static const char* rejecting_approver =
        "@budget($0.05)\n"
        "@replayable\n"
        "@deterministic\n"
        "agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:\n"
        "    return ApprovalDecision(false, \"demo reject-all approver\")\n";

    if (argc != 4) {
        fprintf(stderr, "usage: %s <library> <approver.cor> <expected_hash_hex>\n", argv[0]);
        return 2;
    }

    set_demo_env();
    set_env_var("CORVID_TRACE_PATH", trace_path);

    if (!decode_hex_64(argv[3], expected_hash)) {
        fprintf(stderr, "expected hash must be 64 hex chars\n");
        return 2;
    }

#if defined(_WIN32)
    library = LoadLibraryA(argv[1]);
    if (library == NULL) {
        fprintf(stderr, "failed to load %s\n", argv[1]);
        return 1;
    }
#else
    library = dlopen(argv[1], RTLD_NOW);
    if (library == NULL) {
        fprintf(stderr, "failed to load %s: %s\n", argv[1], dlerror());
        return 1;
    }
#endif

    corvid_abi_verify =
        (corvid_abi_verify_fn)load_symbol(library, "corvid_abi_verify");
    corvid_list_agents =
        (corvid_list_agents_fn)load_symbol(library, "corvid_list_agents");
    corvid_pre_flight =
        (corvid_pre_flight_fn)load_symbol(library, "corvid_pre_flight");
    corvid_call_agent =
        (corvid_call_agent_fn)load_symbol(library, "corvid_call_agent");
    corvid_free_result =
        (corvid_free_result_fn)load_symbol(library, "corvid_free_result");
    corvid_observation_release =
        (corvid_observation_release_fn)load_symbol(library, "corvid_observation_release");
    corvid_register_approver_from_source =
        (corvid_register_approver_from_source_fn)load_symbol(library, "corvid_register_approver_from_source");
    corvid_clear_approver =
        (corvid_clear_approver_fn)load_symbol(library, "corvid_clear_approver");
    if (!corvid_abi_verify || !corvid_list_agents || !corvid_pre_flight ||
        !corvid_call_agent || !corvid_free_result || !corvid_observation_release ||
        !corvid_register_approver_from_source || !corvid_clear_approver) {
        fprintf(stderr, "required approval-bridge symbol missing\n");
        return 1;
    }

    printf("verified_before=%d\n", corvid_abi_verify(expected_hash));

    if (corvid_register_approver_from_source(argv[2], 1.0, &error_message) != CORVID_APPROVER_OK) {
        fprintf(stderr, "approver registration failed: %s\n", error_message == NULL ? "<none>" : error_message);
        corvid_free_result(error_message);
        return 1;
    }

    printf("verified_after_registration=%d\n", corvid_abi_verify(expected_hash));

    agent_count = corvid_list_agents(NULL, 0);
    agents = (CorvidAgentHandle*)calloc(agent_count, sizeof(CorvidAgentHandle));
    if (agents == NULL) {
        fprintf(stderr, "out of memory\n");
        return 1;
    }
    corvid_list_agents(agents, agent_count);
    printf("agent_count=%zu\n", agent_count);
    printf("catalog_has_approver=%d\n", has_agent_named(agents, agent_count, "__corvid_approver"));
    free(agents);

    preflight = corvid_pre_flight("issue_tag", "[\"approved\"]", strlen("[\"approved\"]"));
    printf(
        "preflight_status=%u requires_approval=%u cost_bound_usd=%.2f\n",
        (unsigned)preflight.status,
        (unsigned)preflight.requires_approval,
        preflight.cost_bound_usd);

    memset(&approval, 0, sizeof(approval));
    run_issue_tag_call(
        corvid_call_agent,
        corvid_free_result,
        corvid_observation_release,
        "[\"approved\"]",
        "accept_call",
        &approval);

    if (!path_parent(argv[2], approver_dir, sizeof(approver_dir))) {
        fprintf(stderr, "failed to compute approver dir\n");
        return 1;
    }
    join_path(reject_approver_path, sizeof(reject_approver_path), approver_dir, "approver_reject.cor");
    if (!write_text_file(reject_approver_path, rejecting_approver)) {
        fprintf(stderr, "failed to write rejecting approver source\n");
        return 1;
    }

    error_message = NULL;
    if (corvid_register_approver_from_source(reject_approver_path, 1.0, &error_message) != CORVID_APPROVER_OK) {
        fprintf(stderr, "rejecting approver registration failed: %s\n", error_message == NULL ? "<none>" : error_message);
        corvid_free_result(error_message);
        return 1;
    }

    memset(&approval, 0, sizeof(approval));
    run_issue_tag_call(
        corvid_call_agent,
        corvid_free_result,
        corvid_observation_release,
        "[\"approved\"]",
        "reject_call",
        &approval);

    corvid_clear_approver();

    memset(&approval, 0, sizeof(approval));
    run_issue_tag_call(
        corvid_call_agent,
        corvid_free_result,
        corvid_observation_release,
        "[\"approved\"]",
        "fail_closed_call",
        &approval);
    printf("trace_path=%s\n", trace_path);

    remove(reject_approver_path);
#if defined(_WIN32)
    FreeLibrary(library);
#else
    dlclose(library);
#endif
    return 0;
}
