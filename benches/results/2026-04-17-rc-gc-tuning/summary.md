# RC/GC tuning assessment

## Allocation-pressure scaling

| Target releases / trial | Median orchestration ms | Median GC ms | GC % of orchestration | Median mark count | Median sweep count | Median peak live objects |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `19` | `0.002700` | `0.000200` | `7.4%` | `21` | `21` | `21` |
| `100` | `0.011900` | `0.000600` | `5.0%` | `102` | `102` | `102` |
| `1000` | `0.217950` | `0.027300` | `12.5%` | `1002` | `1002` | `1002` |
| `10000` | `1.707750` | `0.213300` | `12.5%` | `10002` | `10002` | `10002` |
| `100000` | `32.913450` | `10.411400` | `31.6%` | `100002` | `100002` | `100002` |

## GC trigger sensitivity

| GC cadence | Median orchestration ms | Median GC ms | GC % of orchestration | Median GC count | Median peak live objects |
| --- | ---: | ---: | ---: | ---: | ---: |
| `disabled` | `1.780550` | `0.000000` | `0.0%` | `0` | `1` |
| `100` | `2.272000` | `0.052400` | `2.3%` | `1000` | `1` |
| `1000` | `2.246800` | `0.007050` | `0.3%` | `100` | `1` |
| `10000` | `2.544600` | `0.001650` | `0.1%` | `10` | `1` |
| `50000` | `2.508350` | `0.001050` | `0.0%` | `2` | `1` |

## Cycle collector stress

| Cycle pairs / trial | Median orchestration ms | Median GC ms | GC % of orchestration | Median reclaimed cycle objects | Median sweep count | Median peak live objects |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `10` | `0.001950` | `0.000300` | `15.4%` | `20` | `20` | `20` |
| `100` | `0.006700` | `0.002650` | `39.6%` | `200` | `200` | `200` |
| `1000` | `0.070550` | `0.034600` | `49.0%` | `2000` | `2000` | `2000` |
| `10000` | `0.835550` | `0.499650` | `59.8%` | `20000` | `20000` | `20000` |

## Ownership pass at scale

| Target releases / trial | Median retain count | Median release count |
| --- | ---: | ---: |
| `19` | `0` | `21` |
| `100` | `0` | `102` |
| `1000` | `0` | `1002` |
| `10000` | `0` | `10002` |
| `100000` | `0` | `100002` |
