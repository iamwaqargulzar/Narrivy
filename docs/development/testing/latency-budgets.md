# Latency Budgets

Track P50/P95 by machine class:
- trigger→acknowledgement;
- trigger→capture ready;
- speech→first partial;
- release→raw;
- raw→corrected;
- corrected→delivered;
- model cold/warm/hot startup.

Each performance-sensitive PR must state whether it changes these stages and attach benchmark evidence when practical.
