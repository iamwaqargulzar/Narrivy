# Coding and Architecture Style

## Rust
- Edition 2024, stable toolchain.
- Portable crates prefer safe Rust; FFI/unsafe is isolated to adapters with invariants documented beside each unsafe block.
- Prefer newtypes/enums to ambiguous booleans and magic strings.
- Prefer small explicit traits at architectural boundaries; avoid “god traits”.
- Use composition over inheritance-like trait webs.
- Errors are typed and classified into retry/fallback/user-action/policy/invariant categories.
- Production paths do not use `unwrap`, `expect`, or panic for recoverable conditions.
- Async tasks have explicit cancellation and shutdown ownership.
- Do not hold mutex guards over await/blocking calls unless proven safe and necessary.
- Real-time audio callbacks do allocation/blocking only when a platform API makes it unavoidable and the cost has been measured.

## TypeScript/React
- Strict TypeScript.
- Components do not perform platform detection or policy decisions.
- Server/backend state is represented by typed DTOs generated or validated from the application API.
- Design-system primitives own styling; feature components compose them.
- Side effects live in hooks/services, not rendering logic.
- Accessibility semantics are part of component contracts.

## Documentation
Public behavior, persistent schemas and architecture boundaries change in the same PR as code. Avoid comments that restate code; document invariants, rationale, hazards and external API constraints.
