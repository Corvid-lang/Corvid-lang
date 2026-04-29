# Personal Knowledge Agent Runbook

1. Keep `CORVID_LOCAL_ONLY=true` for demo and private mode.
2. Run `corvid check examples/backend/personal_knowledge_agent/src/main.cor`.
3. Apply migrations and load `seeds/demo.sql`.
4. Run `corvid eval examples/backend/personal_knowledge_agent/evals/search_answer_eval.cor`.
5. Inspect `traces/demo.lineage.jsonl` before changing retrieval behavior.
