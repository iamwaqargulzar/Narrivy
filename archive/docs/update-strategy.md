# Update Strategy

Separate:
- application updates;
- model catalog updates;
- model artifact updates;
- plugin updates;
- runtime/accelerator compatibility data.

Never silently replace a user-pinned model with a behaviorally incompatible major revision. Cache invalidation and migration must be explicit.

Auto-update policy must respect package-manager installations and platform conventions.
