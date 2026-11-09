# UI Reference Images — Historical / Design Direction

**Authority:** Supplemental Master Plan Integration Prompt Parts 24, 37-42, 50. These images are **visual and layout reference** only, not pixel-perfect spec. Textual requirements in the supplemental prompt remain authoritative if image detail conflicts with functionality.

**Files:**
- `narrivy-home-dark-approved.png` — owner-approved 2026-09-21 design direction for the production desktop shell and Home screen: dark-first palette, compact icon navigation, local/private status, prominent session action, modular pipeline stages, recent transcript, privacy explanation, keyboard help, and truthful runtime status. Runtime values in the product must still come from live capabilities; values pictured in the concept are illustrative.
- `narrivy-home-reference.png` — Home screen: `Narrivy is ready`, activation hint shown only from the currently qualified/configured trigger (historical mock text used hold-to-talk), `Current configuration` (Compute Auto→Intel NPU, Model Whisper Large-v3 INT8, Mode Clean, Language Auto Detect, Mic Jabra Speak 510, Text Delivery Auto→Direct Insert), `Last transcript` with Copy/Paste Again/Edit, metrics `NPU 14% / 320ms / 2.1GB / Excellent`
- `narrivy-settings-compute-reference.png` — Settings → Compute: sidebar Home/Dictation/Models/Applications/History/Settings/Diagnostics, top Light/Dark toggle, Theme (System/Light/Dark), Automation (Automatic/Manual + Balanced/Best Accuracy/Lowest Latency/Lowest Power), Compute (Device selection Auto/Manual + Automatic priority NPU→dGPU→iGPU→CPU reorderable + Use Vulkan toggle + Automatically fall back + Manual device list + Manual fallback order + Current resolved selection Auto→Intel NPU/OpenVINO/Whisper Large-v3 INT8), Text Delivery (Delivery method dropdown + fallback chain Direct→Clipboard→Portal→Virtual KB→Inbox + clipboard toggles + per-app overrides)

**Reuse:** Implement real responsive accessible interface using actual Tauri/React components and runtime state. Do not copy pixels literally. Improve spacing/accessibility/overflow while retaining direction. Both themes (Light/Dark/System) must pass contrast/focus checks per Parts 37-39. Floating dictation overlay (Part 40) follows same visual language.

**History:** Added 2026-08-21 per owner supplemental prompt. Marked historical/non-authoritative after integration per Part 1 — authoritative spec is merged master in `docs/product/ux/` + `spec/ui/design-tokens.json`.

**Historical correction:** the two binary image contents had been stored under the opposite filenames in the prior pack. the corrected pack swaps the contents so `narrivy-home-reference.png` is the Home visual and `narrivy-settings-compute-reference.png` is the Settings → Compute visual.
