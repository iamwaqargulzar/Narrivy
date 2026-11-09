#!/usr/bin/env python3
"""Rebuild a text-only Hugging Face checkpoint from a Qwen3.5 GGUF file.

Some clean-up models are published only as GGUF (llama.cpp). OpenVINO's
exporter reads Hugging Face checkpoints, so the GGUF tensors are mapped back
to their transformers names and llama.cpp's storage conventions are undone:

- zero-centred RMSNorm weights are stored with 1 added: subtract it again
  (the gated norm inside linear attention is stored as is);
- `A_log` is stored as `-exp(A_log)`: take `log(-a)`;
- the depthwise conv weight is stored squeezed: add the middle axis back.

Only full-precision (BF16/F16/F32) files are accepted: a quantised file
would carry its quantisation error into the converted model. The multi-token
prediction layer is dropped (generation does not use it).

The base model's `config.json` and tokenizer files supply the rest; its
`text_config` becomes the checkpoint's config.

    python scripts/gguf_qwen35_to_hf.py MODEL.gguf BASE_DIR OUT_DIR
"""

import json
import re
import shutil
import sys
from pathlib import Path

import numpy as np
import torch
from gguf import GGUFReader
from safetensors.torch import save_file

ZERO_CENTRED_NORMS = {
    "attn_norm": "input_layernorm",
    "post_attention_norm": "post_attention_layernorm",
}

LINEAR = {
    "attn_qkv": "linear_attn.in_proj_qkv",
    "attn_gate": "linear_attn.in_proj_z",
    "ssm_beta": "linear_attn.in_proj_b",
    "ssm_alpha": "linear_attn.in_proj_a",
    "ssm_out": "linear_attn.out_proj",
}

FULL = {
    "attn_q": "self_attn.q_proj",
    "attn_k": "self_attn.k_proj",
    "attn_v": "self_attn.v_proj",
    "attn_output": "self_attn.o_proj",
}

MLP = {"ffn_gate": "mlp.gate_proj", "ffn_up": "mlp.up_proj", "ffn_down": "mlp.down_proj"}

TOKENIZER_FILES = (
    "tokenizer.json",
    "tokenizer_config.json",
    "vocab.json",
    "merges.txt",
    "chat_template.jinja",
    "LICENSE",
)


def tensor_of(t) -> torch.Tensor:
    kind = t.tensor_type.name
    data = np.asarray(t.data)
    if kind == "BF16":
        return torch.from_numpy(data.view(np.int16).copy()).view(torch.bfloat16)
    if kind == "F16":
        return torch.from_numpy(data.copy()).to(torch.bfloat16)
    if kind == "F32":
        return torch.from_numpy(data.copy())
    raise SystemExit(f"{t.name}: {kind} is quantised; use the BF16/F16 file")


def hf_name(name: str, layers: int) -> tuple[str, str] | None:
    """The transformers name for a GGUF tensor and how to undo its storage."""
    if name == "token_embd.weight":
        return "model.embed_tokens.weight", "as_is"
    if name == "output_norm.weight":
        return "model.norm.weight", "minus_one"
    if name == "output.weight":
        return "lm_head.weight", "as_is"
    m = re.fullmatch(r"blk\.(\d+)\.(.+)\.weight|blk\.(\d+)\.(ssm_a|ssm_dt\.bias)", name)
    if not m:
        raise SystemExit(f"unexpected tensor {name}")
    layer = int(m.group(1) or m.group(3))
    if layer >= layers:
        return None  # the multi-token prediction layer
    stem = m.group(2) or m.group(4)
    prefix = f"model.layers.{layer}."
    if stem in ZERO_CENTRED_NORMS:
        return prefix + ZERO_CENTRED_NORMS[stem] + ".weight", "minus_one"
    if stem in ("attn_q_norm", "attn_k_norm"):
        return prefix + f"self_attn.{stem[5]}_norm.weight", "minus_one"
    if stem == "ssm_norm":
        return prefix + "linear_attn.norm.weight", "as_is"
    if stem == "ssm_a":
        return prefix + "linear_attn.A_log", "log_neg"
    if stem == "ssm_dt.bias":
        return prefix + "linear_attn.dt_bias", "as_is"
    if stem == "ssm_conv1d":
        return prefix + "linear_attn.conv1d.weight", "unsqueeze"
    for table in (LINEAR, FULL, MLP):
        if stem in table:
            return prefix + table[stem] + ".weight", "as_is"
    raise SystemExit(f"unexpected tensor {name}")


def undo(tensor: torch.Tensor, how: str) -> torch.Tensor:
    if how == "minus_one":
        return (tensor.float() - 1.0).to(tensor.dtype)
    if how == "log_neg":
        return torch.log(-tensor.float())
    if how == "unsqueeze":
        return tensor.unsqueeze(1)
    return tensor


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__.strip().splitlines()[-1].strip(), file=sys.stderr)
        return 2
    gguf_path, base, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    reader = GGUFReader(gguf_path)
    if str(reader.fields["general.architecture"].contents()) != "qwen35":
        raise SystemExit("not a Qwen3.5 GGUF")
    base_config = json.loads((base / "config.json").read_text())
    config = dict(base_config["text_config"])
    layers = config["num_hidden_layers"]
    config["architectures"] = ["Qwen3_5ForCausalLM"]
    config["model_type"] = "qwen3_5_text"
    config["dtype"] = "bfloat16"

    state: dict[str, torch.Tensor] = {}
    for t in reader.tensors:
        mapped = hf_name(t.name, layers)
        if mapped is None:
            continue
        key, how = mapped
        state[key] = undo(tensor_of(t), how).contiguous()
    if "lm_head.weight" in state:
        config["tie_word_embeddings"] = False

    expected = 3 + layers * 4  # embeddings, final norm, two norms and MLP per layer...
    if len(state) < expected:
        raise SystemExit(f"only {len(state)} tensors mapped")

    out.mkdir(parents=True, exist_ok=True)
    save_file(state, out / "model.safetensors", metadata={"format": "pt"})
    (out / "config.json").write_text(json.dumps(config, indent=2) + "\n")
    for name in TOKENIZER_FILES:
        if (base / name).is_file():
            shutil.copy2(base / name, out / name)
    print(f"{len(state)} tensors -> {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
