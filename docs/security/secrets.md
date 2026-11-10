# Secrets

API keys and tokens must use platform secret stores when available:
- Linux: Secret Service compatible store;
- Windows: Credential Manager;
- macOS: Keychain.

Configuration stores references/identifiers, not plaintext secrets.

Never log secret values or include them in support bundles. Environment-variable import may be supported for developer/CI use but should not become the normal desktop UX.
