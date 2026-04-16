# Orchestration cost investigation

- Generated: `2026-04-16T20:00:46.787244Z`

## Control baseline

| Stack | Median orchestration ms | Median launcher overhead ms | Control CV % |
|---|---:|---:|---:|
| `corvid` | `42.810` | `73.210` | `15.21` |
| `python` | `0.002` | `221.609` | `26.13` |
| `typescript` | `0.006` | `2902.044` | `32.65` |

## Scenario summaries

### `tool_loop`

- Corvid/Python median ratio: `34.701`
- Corvid/TypeScript median ratio: `3.541`
- Corvid startup proxy share of measured orchestration: `50.3%`
- Corvid median retain/release calls per logical step: `0.00` / `4.75`
- Corvid median GC triggers / safepoints / stack-map entries: `0` / `0` / `23`

| Stack | Median orchestration ms | Median launcher overhead ms | Median actual external wait ms | Median wait bias ms |
|---|---:|---:|---:|---:|
| `corvid` | `85.161` | `99.289` | `491.143` | `26.143` |
| `python` | `2.454` | `234.873` | `466.679` | `1.679` |
| `typescript` | `24.051` | `2996.069` | `488.538` | `23.538` |

| Corvid component | Median ms |
|---|---:|
| `compile_to_ir` | `0.804` |
| `cache_resolve` | `0.204` |
| `binary_exec` | `550.161` |
| `runner_total_wall` | `558.217` |
| `prompt_wait_actual` | `404.232` |
| `tool_wait_actual` | `87.633` |

### `retry_workflow`

- Corvid/Python median ratio: `25.747`
- Corvid/TypeScript median ratio: `2.424`
- Corvid startup proxy share of measured orchestration: `47.9%`
- Corvid median retain/release calls per logical step: `0.00` / `6.00`
- Corvid median GC triggers / safepoints / stack-map entries: `0` / `0` / `5`

| Stack | Median orchestration ms | Median launcher overhead ms | Median actual external wait ms | Median wait bias ms |
|---|---:|---:|---:|---:|
| `corvid` | `89.450` | `75.114` | `363.791` | `28.791` |
| `python` | `3.474` | `239.950` | `337.732` | `2.732` |
| `typescript` | `36.896` | `2955.627` | `371.378` | `36.378` |

| Corvid component | Median ms |
|---|---:|
| `compile_to_ir` | `0.736` |
| `cache_resolve` | `0.200` |
| `binary_exec` | `324.100` |
| `runner_total_wall` | `430.873` |
| `prompt_wait_actual` | `166.027` |
| `tool_wait_actual` | `98.024` |

### `approval_workflow`

- Corvid/Python median ratio: `32.870`
- Corvid/TypeScript median ratio: `2.974`
- Corvid startup proxy share of measured orchestration: `81.8%`
- Corvid median retain/release calls per logical step: `0.00` / `7.00`
- Corvid median GC triggers / safepoints / stack-map entries: `0` / `0` / `7`

| Stack | Median orchestration ms | Median launcher overhead ms | Median actual external wait ms | Median wait bias ms |
|---|---:|---:|---:|---:|
| `corvid` | `52.304` | `97.845` | `241.172` | `-3.828` |
| `python` | `1.591` | `235.240` | `246.184` | `1.184` |
| `typescript` | `17.589` | `2914.660` | `262.297` | `17.297` |

| Corvid component | Median ms |
|---|---:|
| `compile_to_ir` | `0.689` |
| `cache_resolve` | `0.178` |
| `binary_exec` | `297.304` |
| `runner_total_wall` | `303.625` |
| `prompt_wait_actual` | `207.441` |
| `tool_wait_actual` | `33.175` |

### `replay_trace`

- Corvid/Python median ratio: `43.068`
- Corvid/TypeScript median ratio: `3.634`
- Corvid startup proxy share of measured orchestration: `40.6%`
- Corvid median retain/release calls per logical step: `0.00` / `4.75`
- Corvid median GC triggers / safepoints / stack-map entries: `0` / `0` / `18`

| Stack | Median orchestration ms | Median launcher overhead ms | Median actual external wait ms | Median wait bias ms |
|---|---:|---:|---:|---:|
| `corvid` | `105.469` | `119.727` | `457.235` | `32.235` |
| `python` | `2.449` | `312.637` | `426.681` | `1.681` |
| `typescript` | `29.025` | `4023.607` | `453.548` | `28.548` |

| Corvid component | Median ms |
|---|---:|
| `compile_to_ir` | `1.160` |
| `cache_resolve` | `0.336` |
| `binary_exec` | `530.469` |
| `runner_total_wall` | `540.804` |
| `prompt_wait_actual` | `380.583` |
| `tool_wait_actual` | `76.294` |

