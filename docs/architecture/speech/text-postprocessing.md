# Text Post-processing Pipeline

## Position in the speech path

`ASR final → deterministic normalization → dictionary/ITN/spelling/grammar → optional LLM processor → semantic guard → target formatting → delivery`

Stages are independent modules. A Pipeline may skip any optional stage.

## Deterministic baseline

Linux 1.0 must have a fast local logic-based corrector for tasks that do not require generation: spoken punctuation, capitalization, whitespace, locale-aware numbers/dates where deterministic, aliases/dictionary replacements, obvious repeated filler rules, and target-aware spacing.

Candidate open-source technologies can be evaluated for narrower stages (for example ITN, spelling, grammar), but integration is license/footprint/quality driven and not assumed merely because a repository exists.

## Generative post-processing

Optional local or cloud models can implement:

- Faithful Cleanup;
- Polish;
- Rewrite/format-as-email/bullets/etc.

The Pipeline records whether a processor is allowed to change meaning and its edit budget. Faithful Cleanup must preserve protected facts/tokens and fall back to deterministic output when validation fails.

## Semantic guard

Before delivery, compare raw/deterministic/generative variants. Depending on mode, protect or flag changes to numbers, dates, currency, URLs, emails, code/path tokens, quoted spans, and dictionary-protected entities. A large unexplained edit in Faithful Cleanup is rejected or presented for review rather than silently pasted.

## Inference providers

Separate provider transport/auth from text transformation. Provider capability can represent:

- local OpenVINO GenAI or other local runtimes;
- local OpenAI-compatible servers;
- direct cloud APIs;
- routing/gateway providers.

Core does not hard-code provider-specific subscription assumptions. API key/OAuth/browser login support is added only when the provider officially supports that third-party flow.

## Local accelerator sharing

A Pipeline may run both ASR and a post-processing model through OpenVINO or another runtime. Qualification compares co-resident/sequential plans and may split workloads across NPU/dGPU/iGPU/CPU if that improves latency, memory, power, or stability. Keep models warm only when residency budget permits.

## Privacy

Remote transcript permission and remote context permission are separate. Audio is not sent to a text-only provider. Context beyond the transcript is never sent unless the Pipeline requests it and policy explicitly allows it.
