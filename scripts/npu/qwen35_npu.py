#!/usr/bin/env python3
"""Run Qwen3.5 text models on the Intel NPU.

The NPU compiles fixed-shape graphs and cannot schedule the step-by-step loop
that Qwen3.5's linear-attention (Gated DeltaNet) layers export to. This module
rewrites the text model so the NPU can run it:

- every call takes a fixed block of tokens (a prompt block, or one token);
- all memory between calls is explicit: each linear-attention layer's
  convolution window and recurrent state, each full-attention layer's keys
  and values. The host keeps it and feeds it back;
- inside a block, the delta-rule recurrence is solved in closed form with
  matrix products. With N the strictly lower-triangular coupling between the
  block's tokens, the block's updates U solve U = R + N U. N is nilpotent, so
  block-1 applications of U <- R + N U give the exact answer; the graph has
  no loop and no triangular solve. (Squaring N to shorten the chain is exact
  too, but the NPU compiler computed that form wrongly.)

Subcommands:

    check HF_DIR PACK_DIR             compare with transformers on the CPU
    export HF_DIR OUT_DIR [--with-step] [--int4] [--context N]
                                      write the OpenVINO piece(s), INT8 weights
                                      (INT4 with --int4; context 2048 unless N)
    run HF_DIR PACK_DIR NPU_DIR DEVICE CASES
                                      clean the cases with the exported pieces

HF_DIR is a text-only Qwen3.5 checkpoint (see scripts/gguf_qwen35_to_hf.py).
"""

import json
import math
import sys
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F
from safetensors.torch import load_file

PROMPT_BLOCK = 16
# Keys and values kept per full-attention layer. A clean-up holds the system
# prompt (about 240 tokens for SpeakoFlow), the dictation and the reply, which
# is about as long; 2048 fits a five-minute dictation (512 failed above about
# 120 words).
CONTEXT = 2048
MASKED = -1.0e4  # finite, so a row with nothing to attend never makes NaN


class Shape:
    def __init__(self, config: dict):
        self.hidden = config["hidden_size"]
        self.layers = config["num_hidden_layers"]
        # Qwen3.5 mixes linear-attention layers in; Qwen3 has none.
        self.kinds = config.get("layer_types") or ["full_attention"] * self.layers
        self.qwen35 = config["model_type"].startswith("qwen3_5")
        # Qwen3.5 stores norms zero-centred and gates attention output.
        self.centred = self.qwen35
        self.eps = config["rms_norm_eps"]
        self.heads = config["num_attention_heads"]
        self.kv_heads = config["num_key_value_heads"]
        self.head_dim = config["head_dim"]
        rope = config.get("rope_parameters") or {"rope_theta": config["rope_theta"]}
        self.rotary = int(self.head_dim * rope.get("partial_rotary_factor", 1.0))
        self.theta = rope["rope_theta"]
        self.vocab = config["vocab_size"]
        # Set by `export --mtp`: the piece also runs the model's MTP layer.
        self.mtp = False
        self.conv_kernel = 1
        if "linear_attention" in self.kinds:
            self.lin_heads = config["linear_num_value_heads"]
            self.lin_k = config["linear_key_head_dim"]
            self.lin_v = config["linear_value_head_dim"]
            self.conv_kernel = config["linear_conv_kernel_dim"]
            # Bigger models share each query/key head between several value
            # heads (Qwen3.5-4B: 16 for 32).
            self.lin_k_heads = config["linear_num_key_heads"]
            self.conv_dim = 2 * self.lin_k * self.lin_k_heads + self.lin_v * self.lin_heads
            if self.lin_heads % self.lin_k_heads:
                raise SystemExit("value heads must be a multiple of key heads")

    @property
    def linear(self) -> list[int]:
        return [i for i, kind in enumerate(self.kinds) if kind == "linear_attention"]

    @property
    def full(self) -> list[int]:
        return [i for i, kind in enumerate(self.kinds) if kind == "full_attention"]


def rms(x: torch.Tensor, weight: torch.Tensor, eps: float, centred: bool) -> torch.Tensor:
    y = x * torch.rsqrt(x.pow(2).mean(-1, keepdim=True) + eps)
    return y * (1.0 + weight) if centred else y * weight


class Linear(torch.nn.Module):
    """A projection that keeps its weight in the checkpoint's own type
    (bf16): a 4B model would need 16 GB as f32. Activations stay f32."""

    def __init__(self, weight: torch.Tensor):
        super().__init__()
        self.weight = torch.nn.Parameter(weight, requires_grad=False)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        # The weight is widened, not the activation: in the exported graph
        # that is a constant conversion the weight compressor folds away.
        return F.linear(x, self.weight.to(x.dtype))


def linear(weight: torch.Tensor) -> Linear:
    return Linear(weight)


class Static(torch.nn.Module):
    """The text model for one fixed block size."""

    def __init__(self, weights: dict, shape: Shape, block: int):
        super().__init__()
        self.s = shape
        self.block = block
        # Matrices keep their own type; small vectors become f32.
        w = {k: v if v.dim() == 2 else v.float() for k, v in weights.items()}
        head = w.get("lm_head.weight", w["model.embed_tokens.weight"])
        self.lm_head = linear(head)
        # With tied embeddings the input lookup reads the output layer's own
        # weight, so the exported model holds one compressed table, not a
        # compressed one and an uncompressed one.
        self.tied = "lm_head.weight" not in w
        if not self.tied:
            self.embed = torch.nn.Embedding.from_pretrained(w["model.embed_tokens.weight"], freeze=True)
        self.register_buffer("norm", w["model.norm.weight"])
        self.mods = torch.nn.ModuleDict()
        self.params = torch.nn.ParameterDict()
        for i in range(shape.layers):
            p = f"model.layers.{i}."
            for name in ("mlp.gate_proj", "mlp.up_proj", "mlp.down_proj"):
                self.mods[f"{i}_{name.split('.')[1]}"] = linear(w[p + name + ".weight"])
            self.params[f"{i}_in_norm"] = torch.nn.Parameter(w[p + "input_layernorm.weight"], requires_grad=False)
            self.params[f"{i}_post_norm"] = torch.nn.Parameter(
                w[p + "post_attention_layernorm.weight"], requires_grad=False
            )
            if shape.kinds[i] == "linear_attention":
                for name in ("in_proj_qkv", "in_proj_z", "in_proj_b", "in_proj_a", "out_proj"):
                    self.mods[f"{i}_{name}"] = linear(w[p + f"linear_attn.{name}.weight"])
                self.params[f"{i}_conv"] = torch.nn.Parameter(
                    w[p + "linear_attn.conv1d.weight"][:, 0, :], requires_grad=False
                )
                self.params[f"{i}_A"] = torch.nn.Parameter(
                    -torch.exp(w[p + "linear_attn.A_log"]), requires_grad=False
                )
                self.params[f"{i}_dt"] = torch.nn.Parameter(w[p + "linear_attn.dt_bias"], requires_grad=False)
                self.params[f"{i}_onorm"] = torch.nn.Parameter(
                    w[p + "linear_attn.norm.weight"], requires_grad=False
                )
            else:
                for name in ("q_proj", "k_proj", "v_proj", "o_proj"):
                    self.mods[f"{i}_{name}"] = linear(w[p + f"self_attn.{name}.weight"])
                self.params[f"{i}_qn"] = torch.nn.Parameter(w[p + "self_attn.q_norm.weight"], requires_grad=False)
                self.params[f"{i}_kn"] = torch.nn.Parameter(w[p + "self_attn.k_norm.weight"], requires_grad=False)
        b = block
        self.register_buffer(
            "inv_freq",
            1.0 / (shape.theta ** (torch.arange(0, shape.rotary, 2, dtype=torch.float32) / shape.rotary)),
        )
        # Cumulative sums and causal masks as constant matrices.
        self.register_buffer("upper_incl", torch.triu(torch.ones(b, b)))  # [s, t]: s <= t
        self.register_buffer("lower_strict", torch.tril(torch.ones(b, b), -1))
        self.register_buffer("lower_incl", torch.tril(torch.ones(b, b)))
        # The MTP layer drafts the token after each position's choice, in
        # the same call (its keys and values are two more memory slots).
        self.mtp = Mtp(weights, shape, block, head=self.lm_head) if shape.mtp else None

    def delta_block(self, i, x, conv, state, valid, conv_idx):
        s = self.s
        b = self.block
        qkv = self.mods[f"{i}_in_proj_qkv"](x)  # [b, conv_dim]
        seq = torch.cat([conv.transpose(0, 1), qkv], 0)  # [k-1 + b, conv_dim]
        weight = self.params[f"{i}_conv"]  # [conv_dim, kernel]
        mixed = sum(seq[j : j + b] * weight[:, j] for j in range(s.conv_kernel))
        mixed = F.silu(mixed)
        new_conv = torch.index_select(seq, 0, conv_idx).transpose(0, 1)
        kd = s.lin_k * s.lin_k_heads
        q = mixed[:, :kd].reshape(b, s.lin_k_heads, s.lin_k).transpose(0, 1)
        k = mixed[:, kd : 2 * kd].reshape(b, s.lin_k_heads, s.lin_k).transpose(0, 1)
        v = mixed[:, 2 * kd :].reshape(b, s.lin_heads, s.lin_v).transpose(0, 1)
        if s.lin_heads != s.lin_k_heads:
            share = s.lin_heads // s.lin_k_heads
            q, k = q.repeat_interleave(share, 0), k.repeat_interleave(share, 0)
        q = q * torch.rsqrt((q * q).sum(-1, keepdim=True) + 1e-6) / math.sqrt(s.lin_k)
        k = k * torch.rsqrt((k * k).sum(-1, keepdim=True) + 1e-6)
        z = self.mods[f"{i}_in_proj_z"](x).reshape(b, s.lin_heads, s.lin_v)
        beta = torch.sigmoid(self.mods[f"{i}_in_proj_b"](x)) * valid[:, None]  # [b, h]
        g = self.params[f"{i}_A"] * F.softplus(self.mods[f"{i}_in_proj_a"](x) + self.params[f"{i}_dt"])
        g = (g * valid[:, None]).transpose(0, 1)  # [h, b]
        beta = beta.transpose(0, 1)
        gamma = g @ self.upper_incl  # cumulative decay [h, b]
        diff = gamma[:, :, None] - gamma[:, None, :]  # [h, t, s]
        decay_strict = torch.exp(diff * self.lower_strict + MASKED * (1 - self.lower_strict))
        decay_incl = torch.exp(diff * self.lower_incl + MASKED * (1 - self.lower_incl))
        e_gamma = torch.exp(gamma)
        rhs = beta[:, :, None] * (v - e_gamma[:, :, None] * (k @ state))
        n = -(beta[:, :, None] * (k @ k.transpose(1, 2)) * decay_strict)
        u = rhs
        for _ in range(b - 1):
            u = rhs + n @ u
        out = e_gamma[:, :, None] * (q @ state) + ((q @ k.transpose(1, 2)) * decay_incl) @ u
        last = gamma[:, -1]
        new_state = torch.exp(last)[:, None, None] * state + k.transpose(1, 2) @ (
            torch.exp(last[:, None] - gamma)[:, :, None] * u
        )
        out = out.transpose(0, 1)  # [b, h, v]
        out = rms(out, self.params[f"{i}_onorm"], s.eps, centred=False) * F.silu(z)
        return self.mods[f"{i}_out_proj"](out.reshape(b, -1)), new_conv, new_state

    def attention(self, i, x, past_k, past_v, cos, sin, mask):
        s = self.s
        b = self.block
        if s.qwen35:
            qg = self.mods[f"{i}_q_proj"](x).reshape(b, s.heads, 2 * s.head_dim)
            q, gate = qg[..., : s.head_dim], qg[..., s.head_dim :].reshape(b, -1)
        else:
            q, gate = self.mods[f"{i}_q_proj"](x).reshape(b, s.heads, s.head_dim), None
        q = rms(q, self.params[f"{i}_qn"], s.eps, centred=s.centred)
        k = rms(
            self.mods[f"{i}_k_proj"](x).reshape(b, s.kv_heads, s.head_dim), self.params[f"{i}_kn"], s.eps, s.centred
        )
        v = self.mods[f"{i}_v_proj"](x).reshape(b, s.kv_heads, s.head_dim)
        q, k = self.rotate(q, cos, sin), self.rotate(k, cos, sin)
        k, v = k.transpose(0, 1), v.transpose(0, 1)  # [kv, b, d]
        keys = torch.cat([past_k, k], 1).repeat_interleave(s.heads // s.kv_heads, 0)
        values = torch.cat([past_v, v], 1).repeat_interleave(s.heads // s.kv_heads, 0)
        scores = q.transpose(0, 1) @ keys.transpose(1, 2) / math.sqrt(s.head_dim) + mask[None]
        out = (torch.softmax(scores, -1) @ values).transpose(0, 1).reshape(b, -1)
        if gate is not None:
            out = out * torch.sigmoid(gate)
        return self.mods[f"{i}_o_proj"](out), k, v

    def rotate(self, x, cos, sin):
        r = self.s.rotary
        rot, rest = x[..., :r], x[..., r:]
        half = r // 2
        turned = torch.cat([-rot[..., half:], rot[..., :half]], -1)
        return torch.cat([rot * cos[:, None] + turned * sin[:, None], rest], -1)

    def forward(self, ids, pos, valid, conv_idx, last, mask, *memory):
        s = self.s
        if self.tied:
            h = torch.index_select(self.lm_head.weight, 0, ids).float()
        else:
            h = self.embed(ids).float()
        freqs = pos.float()[:, None] * self.inv_freq[None]
        emb = torch.cat([freqs, freqs], -1)
        cos, sin = torch.cos(emb).to(h.dtype), torch.sin(emb).to(h.dtype)
        kept = memory[0].dtype  # memory crosses calls in this type (f16 on the NPU)
        it = (m.float() for m in memory)
        out_memory = []
        for i in range(s.layers):
            x = rms(h, self.params[f"{i}_in_norm"], s.eps, centred=s.centred)
            if s.kinds[i] == "linear_attention":
                y, conv, state = self.delta_block(i, x, next(it), next(it), valid, conv_idx)
                out_memory += [conv, state]
            else:
                y, k, v = self.attention(i, x, next(it), next(it), cos, sin, mask)
                out_memory += [k, v]
            h = h + y
            x = rms(h, self.params[f"{i}_post_norm"], s.eps, centred=s.centred)
            m = self.mods
            h = h + m[f"{i}_down_proj"](F.silu(m[f"{i}_gate_proj"](x)) * m[f"{i}_up_proj"](x))
        self.last_hidden = h  # before the final norm (read by tests)
        logits = self.lm_head(rms(h, self.norm, s.eps, centred=s.centred))
        # Greedy choices on the device: numbers come back, not the vocabulary.
        # `tokens` is the choice after every position, which lets the runner
        # check guessed tokens in one call; `token` is the one after `last`.
        tokens = torch.argmax(logits, -1)
        token = torch.index_select(tokens, 0, last)
        if self.mtp is None:
            return (token, tokens, *(m.to(kept) for m in out_memory))
        # MTP entry at each position: (that position's choice, its hidden
        # state) -> the token after the choice. Rows past what the runner
        # accepts are simply not counted, like keys and values.
        drafts, _, mtp_k, mtp_v = self.mtp(tokens, h, pos + 1, mask, next(it), next(it))
        out_memory += [mtp_k, mtp_v]
        return (token, tokens, drafts, *(m.to(kept) for m in out_memory))


class Mtp(Static):
    """Qwen3.5's multi-token-prediction layer, for drafting tokens.

    From the main model's hidden state at position t and the token at t+1
    it predicts the token at t+2: the token's embedding and the hidden state
    are each normed, joined, projected back to the model width, passed
    through one attention layer with its own keys and values, normed and
    scored with the shared output layer. Its own output hidden state stands
    in for the main model's when drafting further ahead.
    """

    def __init__(self, weights: dict, shape: Shape, block: int, embed_first: bool = True, head=None):
        import copy

        one = copy.copy(shape)
        one.layers, one.kinds, one.mtp = 1, ["full_attention"], False
        mapped = {k.replace("mtp.layers.0.", "model.layers.0.", 1): v for k, v in weights.items() if k.startswith("mtp.layers.0.")}
        mapped["model.norm.weight"] = weights["mtp.norm.weight"]
        mapped["model.embed_tokens.weight"] = weights["model.embed_tokens.weight"]
        if "lm_head.weight" in weights:
            mapped["lm_head.weight"] = weights["lm_head.weight"]
        super().__init__(mapped, one, block)
        if head is not None:
            # Inside the main piece: score with the main model's own output
            # layer, so the exported model keeps one vocabulary table.
            self.lm_head = head
        self.fc = linear(weights["mtp.fc.weight"])
        self.register_buffer("pre_embed", weights["mtp.pre_fc_norm_embedding.weight"].float())
        self.register_buffer("pre_hidden", weights["mtp.pre_fc_norm_hidden.weight"].float())
        self.embed_first = embed_first

    def forward(self, ids, hidden, pos, mask, past_k, past_v):
        s = self.s
        kept = past_k.dtype
        e = torch.index_select(self.lm_head.weight, 0, ids).float() if self.tied else self.embed(ids).float()
        e = rms(e, self.pre_embed, s.eps, centred=s.centred)
        g = rms(hidden.float(), self.pre_hidden, s.eps, centred=s.centred)
        h = self.fc(torch.cat([e, g] if self.embed_first else [g, e], -1))
        freqs = pos.float()[:, None] * self.inv_freq[None]
        emb = torch.cat([freqs, freqs], -1)
        cos, sin = torch.cos(emb), torch.sin(emb)
        x = rms(h, self.params["0_in_norm"], s.eps, centred=s.centred)
        y, k, v = self.attention(0, x, past_k.float(), past_v.float(), cos, sin, mask)
        h = h + y
        x = rms(h, self.params["0_post_norm"], s.eps, centred=s.centred)
        m = self.mods
        h = h + m["0_down_proj"](F.silu(m["0_gate_proj"](x)) * m["0_up_proj"](x))
        tokens = torch.argmax(self.lm_head(rms(h, self.norm, s.eps, centred=s.centred)), -1)
        return tokens, h.to(kept), k.to(kept), v.to(kept)


def memory_shapes(shape: Shape) -> dict[str, tuple]:
    dims = {}
    for i in range(shape.layers):
        if shape.kinds[i] == "linear_attention":
            dims[f"conv.{i}"] = (shape.conv_dim, shape.conv_kernel - 1)
            dims[f"state.{i}"] = (shape.lin_heads, shape.lin_k, shape.lin_v)
        else:
            dims[f"key.{i}"] = (shape.kv_heads, CONTEXT, shape.head_dim)
            dims[f"value.{i}"] = (shape.kv_heads, CONTEXT, shape.head_dim)
    if shape.mtp:
        dims["key.mtp"] = (shape.kv_heads, CONTEXT, shape.head_dim)
        dims["value.mtp"] = (shape.kv_heads, CONTEXT, shape.head_dim)
    return dims


def fixed_inputs(length: int, tokens: list[int], block: int, kernel: int) -> list[np.ndarray]:
    """Token ids, positions, which slots are real, the convolution window
    to keep, the row to answer from and the attention mask."""
    n = len(tokens)
    if length + n > CONTEXT:
        raise RuntimeError("context full")
    ids = np.zeros(block, np.int64)
    ids[:n] = tokens
    pos = np.arange(length, length + block, dtype=np.int64)
    valid = (np.arange(block) < n).astype(np.float32)
    conv_idx = np.arange(n, n + kernel, dtype=np.int64)
    last = np.array([n - 1], np.int64)
    mask = np.full((block, CONTEXT + block), MASKED, np.float32)
    mask[:, :length] = 0.0
    for t in range(block):
        mask[t, CONTEXT : CONTEXT + min(t, n - 1) + 1] = 0.0
    return [ids, pos, valid, conv_idx, last, mask]


FIXED = ["ids", "pos", "valid", "conv_idx", "last", "mask"]


def memory_names(shape: Shape) -> list[str]:
    return list(memory_shapes(shape))


class Memory:
    """What the host keeps between calls."""

    def __init__(self, shape: Shape, dtype=np.float32):
        self.s = shape
        self.length = 0
        self.slots = {name: np.zeros(dims, dtype) for name, dims in memory_shapes(shape).items()}

    def copy(self) -> "Memory":
        other = Memory.__new__(Memory)
        other.s, other.length = self.s, self.length
        other.slots = {k: v.copy() for k, v in self.slots.items()}
        return other

    def inputs(self, tokens: list[int], block: int) -> list[np.ndarray]:
        fixed = fixed_inputs(self.length, tokens, block, self.s.conv_kernel - 1)
        return fixed + [self.slots[k] for k in memory_names(self.s)]

    def update(self, n: int, outputs: list[np.ndarray]) -> np.ndarray:
        names = memory_names(self.s)
        for name, value in zip(names, outputs[len(outputs) - len(names) :]):
            if name.startswith(("key.", "value.")):
                self.slots[name][:, self.length : self.length + n] = value[:, :n]
            else:
                self.slots[name] = np.ascontiguousarray(value)
        self.length += n
        return int(outputs[0][0])


class Runner:
    """Greedy generation with the two fixed-size pieces."""

    def __init__(self, shape: Shape, prompt_call, step_call):
        self.s, self.prompt_call, self.step_call = shape, prompt_call, step_call

    def feed(self, memory: Memory, tokens: list[int]) -> int:
        token = -1
        for start in range(0, len(tokens), PROMPT_BLOCK):
            chunk = tokens[start : start + PROMPT_BLOCK]
            if len(chunk) == 1:
                token = memory.update(1, self.step_call(memory.inputs(chunk, 1)))
            else:
                token = memory.update(len(chunk), self.prompt_call(memory.inputs(chunk, PROMPT_BLOCK)))
        return token

    def generate(self, memory: Memory, tokens: list[int], stop: set[int], limit: int) -> list[int]:
        token = self.feed(memory, tokens)
        out = []
        for _ in range(limit):
            if token in stop:
                break
            out.append(token)
            token = memory.update(1, self.step_call(memory.inputs([token], 1)))
        return out


class Bound:
    """The device runner: memory stays in tensors the device reads in place.

    Each call's state outputs are written straight into the second of two
    buffers, which becomes the next call's input; keys and values are kept
    in one buffer and only the new rows are written. Nothing round-trips
    through copies per token.
    """

    def __init__(self, core, npu_dir: Path, device: str, config: dict):
        import openvino as ov

        self.ov = ov
        self.requests = {}
        for file in ("prompt", "step"):
            if not (npu_dir / f"{file}.xml").is_file():
                continue
            started = time.time()
            compiled = core.compile_model(npu_dir / f"{file}.xml", device, config)
            print(f"compile {file} on {device}: {time.time() - started:.1f}s", flush=True)
            self.requests[file] = compiled.create_infer_request()
        # Names and shapes from the file: compiled ports can carry aliases.
        model = core.read_model(npu_dir / "prompt.xml").inputs
        self.dims = {p.get_any_name(): (p.get_element_type(), list(p.get_shape())) for p in model}
        try:
            context = core.get_default_context(device)
            make = lambda t, d: context.create_host_tensor(t, ov.Shape(d))  # noqa: E731
        except RuntimeError:
            make = lambda t, d: ov.Tensor(t, ov.Shape(d))  # noqa: E731
        self.names = [n for n in self.dims if n not in FIXED]
        outputs = {n for p in core.read_model(npu_dir / "prompt.xml").outputs for n in p.get_names()}
        self.drafting = "drafts" in outputs
        self.last_drafts: list[int] = []
        convs = [n for n in self.names if n.startswith("conv.")]
        self.kernel = self.dims[convs[0]][1][1] if convs else 0
        self.front = {n: make(*self.dims[n]) for n in self.names}
        self.back = {n: make(*self.dims[n]) for n in self.names if n.startswith(("conv.", "state."))}
        self.length = 0
        self.bind()

    def bind(self):
        for request in self.requests.values():
            for name, tensor in self.front.items():
                request.set_tensor(name, tensor)
            for name, tensor in self.back.items():
                request.set_tensor(f"new_{name}", tensor)

    def clear(self):
        for tensor in [*self.front.values(), *self.back.values()]:
            tensor.data[...] = 0
        self.length = 0

    def snapshot(self):
        """The recurrent state only: calls write keys and values only at or
        past the current length, so earlier rows stay as they were."""
        return self.length, {n: self.front[n].data.copy() for n in self.back}

    def restore(self, snap):
        self.length, saved = snap
        for name, value in saved.items():
            self.front[name].data[...] = value

    def call(self, tokens: list[int]) -> int:
        n = len(tokens)
        file, block = ("step", 1) if n == 1 and "step" in self.requests else ("prompt", PROMPT_BLOCK)
        request = self.requests[file]
        for name, value in zip(FIXED, fixed_inputs(self.length, tokens, block, self.kernel)):
            if name in self.dims:
                request.get_tensor(name).data[...] = value
        request.infer()
        for name in self.names:
            if name.startswith(("key.", "value.")):
                rows = request.get_tensor(f"new_{name}").data
                self.front[name].data[:, self.length : self.length + n] = rows[:, :n]
        # The state just written becomes the next input; the old input
        # buffer takes the next output. Rebinding only swaps pointers.
        spent = {name: self.front[name] for name in self.back}
        self.front.update(self.back)
        self.back = spent
        self.bind()
        self.length += n
        self.last_choices = [int(t) for t in request.get_tensor("tokens").data.reshape(-1)]
        if self.drafting:
            self.last_drafts = [int(t) for t in request.get_tensor("drafts").data.reshape(-1)]
        return int(request.get_tensor("token").data.reshape(-1)[0])

    def feed(self, tokens: list[int]) -> int:
        token = -1
        for start in range(0, len(tokens), PROMPT_BLOCK):
            token = self.call(tokens[start : start + PROMPT_BLOCK])
        return token

    def generate(self, tokens: list[int], stop: set[int], limit: int, source: list[int] = ()) -> list[int]:
        """Greedy generation that guesses ahead from `source` (the dictated
        text): each call feeds the next token plus a guess of what follows,
        and keeps every guessed token the model agrees with.

        Keys and values past the agreed tokens are simply not counted.
        Recurrent state cannot be rewound, so for models that have it the
        call starts from the last state known good, and tokens agreed since
        then ride along at the front of the next call ("pending"), together
        with its guess: no call is spent on rolling back."""
        token = self.feed(tokens)
        # The MTP draft for the token after `token`, when the piece has one.
        last_fed = (len(tokens) - 1) % PROMPT_BLOCK
        draft = self.last_drafts[last_fed] if self.drafting else None
        out: list[int] = []
        rolling = any(n.startswith(("conv.", "state.")) for n in self.names)
        good = self.snapshot() if rolling else None
        pending: list[int] = []
        self.calls = 0
        while len(out) < limit and token not in stop:
            room = PROMPT_BLOCK - 1 - len(pending)
            room = min(room, limit - len(out) - 1, CONTEXT - self.length - len(pending) - 1)
            guess = []
            if room > 0 and draft is not None:
                guess = [draft] + lookup(list(source), out + [token, draft], room - 1)
            elif room > 0:
                guess = lookup(list(source), out + [token], room)
            fed = pending + [token] + guess
            self.call(fed)
            self.calls += 1
            choices = self.last_choices[len(pending) :]
            agreed = 0
            while agreed < len(guess) and choices[agreed] == guess[agreed] and guess[agreed] not in stop:
                agreed += 1
            # Read before any settling call overwrites it: the draft for the
            # token after the next one.
            next_draft = self.last_drafts[len(pending) + agreed] if self.drafting else None
            out += [token] + guess[:agreed]
            if agreed == len(guess):
                pending = []
                if rolling:
                    good = self.snapshot()
            elif rolling:
                pending = pending + [token] + guess[:agreed]
                self.restore(good)
                if len(pending) >= PROMPT_BLOCK - 1:
                    # Too many to carry: settle them now.
                    self.call(pending)
                    self.calls += 1
                    pending = []
                    good = self.snapshot()
            else:
                self.length -= len(guess) - agreed
            token = choices[agreed]
            draft = next_draft
        return out


def lookup(source: list[int], recent: list[int], most: int) -> list[int]:
    """What followed the latest place `source` has the end of `recent`:
    the longest match of the last three, two or one tokens wins."""
    for size in (3, 2, 1):
        if len(recent) < size:
            continue
        tail = recent[-size:]
        for start in range(len(source) - size, -1, -1):
            if source[start : start + size] == tail:
                return source[start + size : start + size + most]
    return []


def stop_tokens(tokenizer) -> set[int]:
    names = ("<|im_end|>", "<|endoftext|>")
    return {tokenizer.convert_tokens_to_ids(n) for n in names} | {tokenizer.eos_token_id}


def load(hf_dir: Path):
    """The text model's shape and weights. A vision-language checkpoint
    (Qwen3.5-4B) keeps the text model under `model.language_model`; its
    vision tower is left out; its multi-token-prediction layer (`mtp.*`)
    is kept for `export --mtp`."""
    config = json.loads((hf_dir / "config.json").read_text())
    if "text_config" in config:
        tied = config.get("tie_word_embeddings", config["text_config"].get("tie_word_embeddings"))
        config = {**config["text_config"], "tie_word_embeddings": tied}
    index = hf_dir / "model.safetensors.index.json"
    files = sorted(set(json.loads(index.read_text())["weight_map"].values())) if index.is_file() else ["model.safetensors"]
    weights = {}
    for file in files:
        for name, tensor in load_file(hf_dir / file).items():
            if name.startswith("model.visual."):
                continue
            weights[name.replace("model.language_model.", "model.", 1)] = tensor
    return Shape(config), weights


def prompt_parts(tokenizer, system: str, text: str) -> tuple[list[int], list[int]]:
    """The fixed system part (computed once) and the per-dictation rest."""
    full = tokenizer.apply_chat_template(
        [{"role": "system", "content": system}, {"role": "user", "content": text}],
        add_generation_prompt=True,
        enable_thinking=False,
        tokenize=False,
    )
    head = full[: full.index("<|im_start|>user")]
    head_ids = tokenizer(head, add_special_tokens=False)["input_ids"]
    full_ids = tokenizer(full, add_special_tokens=False)["input_ids"]
    if full_ids[: len(head_ids)] != head_ids:
        raise SystemExit("system prompt does not tokenize on its own")
    return head_ids, full_ids[len(head_ids) :]


def system_prompt(pack_dir: Path) -> tuple[str, str]:
    """The pack's system prompt and the prefix before the dictated text."""
    profile = pack_dir / "narrivy-refine.json"
    if profile.is_file():
        value = json.loads(profile.read_text())
        return value["system"], value.get("user_prefix", "")
    raise SystemExit(f"no narrivy-refine.json in {pack_dir}")


def torch_call(module):
    def call(inputs):
        with torch.no_grad():
            outs = module(*[torch.from_numpy(np.asarray(x)) for x in inputs])
        return [o.numpy() for o in outs]

    return call


def check(hf_dir: Path, pack_dir: Path) -> int:
    from transformers import AutoModelForCausalLM, AutoTokenizer

    shape, weights = load(hf_dir)
    weights = {k: v.float() for k, v in weights.items()}  # the reference runs in f32
    tokenizer = AutoTokenizer.from_pretrained(hf_dir)
    system, prefix = system_prompt(pack_dir)
    reference = AutoModelForCausalLM.from_pretrained(hf_dir, dtype=torch.float32)
    runner = Runner(shape, torch_call(Static(weights, shape, PROMPT_BLOCK)), torch_call(Static(weights, shape, 1)))
    stop = stop_tokens(tokenizer)
    texts = [
        "Let's ship it Thursday, no, Friday.",
        "Two things. Bullet one, call the vendor. Bullet two, send the invoice.",
        "so um i need to like send the the report by uh friday",
    ]
    same = 0
    for text in texts:
        head, rest = prompt_parts(tokenizer, system, prefix + text)
        ids = torch.tensor([head + rest])
        expected = reference.generate(ids, max_new_tokens=60, do_sample=False)[0, ids.shape[1] :].tolist()
        cut = next((n for n, t in enumerate(expected) if t in stop), len(expected))
        expected = expected[:cut]
        memory = Memory(shape)
        runner.feed(memory, head)
        got = runner.generate(memory, rest, stop, 60)
        ok = got == expected
        same += ok
        print("SAME" if ok else "DIFF", repr(tokenizer.decode(got)), "|", repr(tokenizer.decode(expected)))
    print(f"{same}/{len(texts)} identical")
    return 0 if same == len(texts) else 1


def export(hf_dir: Path, out: Path, with_step: bool = False, int4: bool = False, mtp: bool = False) -> int:
    import nncf
    import openvino as ov

    shape, weights = load(hf_dir)
    out.mkdir(parents=True, exist_ok=True)
    shape.mtp = mtp
    if mtp and "mtp.fc.weight" not in weights:
        raise SystemExit("this checkpoint has no MTP layer")
    names = memory_names(shape)
    # One piece does everything: a block costs little more than one token
    # (the weights are read once per call either way), and a second piece
    # would hold a second copy of the weights in memory. `--with-step` adds
    # the one-token piece anyway.
    pieces = [(PROMPT_BLOCK, "prompt")] + ([(1, "step")] if with_step else [])
    for block, file in pieces:
        module = Static(weights, shape, block).eval()
        memory = Memory(shape, np.float16)  # memory crosses calls in f16
        example = [torch.from_numpy(x) for x in memory.inputs([0] * block, block)]
        started = time.time()
        with torch.no_grad():
            model = ov.convert_model(module, example_input=tuple(example))
        # Unused inputs are pruned by the converter: name what remains by
        # matching it to the example it came from.
        all_names = ["ids", "pos", "valid", "conv_idx", "last", "mask", *names]
        kept = list(range(len(all_names)))
        if len(model.inputs) != len(kept):
            unused = () if "linear_attention" in shape.kinds else ("valid", "conv_idx")
            kept = [i for i, name in enumerate(all_names) if name not in unused]
        if len(model.inputs) != len(kept):
            raise SystemExit(f"{len(model.inputs)} inputs after export, expected {len(kept)}")
        example = [example[i] for i in kept]
        present = [all_names[i] for i in kept if all_names[i] in FIXED]
        for port, name in zip(model.inputs, [all_names[i] for i in kept]):
            port.get_tensor().set_names({name})
        for port, name in zip(model.outputs, ["token", "tokens", *(["drafts"] if shape.mtp else []), *[f"new_{n}" for n in names]]):
            port.get_tensor().set_names({name})
        # The NPU compiles fixed shapes only.
        model.reshape({port: ov.PartialShape(list(x.shape)) for port, x in zip(model.inputs, example)})
        if int4:
            # Group-wise symmetric INT4 for the projections (the NPU's own
            # format); NNCF keeps embeddings and the output layer at INT8.
            # A layer whose width 128 does not divide gets the nearest group
            # size that does. The linear-attention gate projections (one
            # output per head) stay in f16: in INT4 the NPU's result for the
            # decay gate drifts far enough (45-80% on the state) to derail
            # the model, and they are tiny.
            model = nncf.compress_weights(
                model,
                mode=nncf.CompressWeightsMode.INT4_SYM,
                group_size=128,
                ratio=1.0,
                ignored_scope=nncf.IgnoredScope(names=gate_projections(model, shape), validate=False),
                advanced_parameters=nncf.AdvancedCompressionParameters(
                    group_size_fallback_mode=nncf.GroupSizeFallbackMode.ADJUST
                ),
            )
        else:
            model = nncf.compress_weights(model, mode=nncf.CompressWeightsMode.INT8_SYM)
        ov.save_model(model, out / f"{file}.xml", compress_to_fp16=True)
        print(f"{file}: {time.time() - started:.0f}s")
    layout = {
        "prompt_block": PROMPT_BLOCK,
        "context": CONTEXT,
        "kernel": shape.conv_kernel - 1,
        "memory": [{"name": n, "shape": list(d)} for n, d in memory_shapes(shape).items()],
        # Inputs a model does not use are dropped at export (Qwen3 has no
        # linear attention, so no `valid` or `conv_idx`).
        "inputs": present,
    }
    (out / "npu-shape.json").write_text(json.dumps(layout, indent=2) + "\n")
    write_tokenizer(hf_dir, out)
    return 0


def gate_projections(model, shape: Shape) -> list[str]:
    """The MatMuls of the linear-attention gates: a [heads, hidden] weight."""
    names = []
    if "linear_attention" not in shape.kinds:
        return names
    for op in model.get_ops():
        if op.get_type_name() != "MatMul":
            continue
        source = op.input_value(1).get_node()
        while source.get_type_name() == "Convert":
            source = source.input_value(0).get_node()
        if source.get_type_name() == "Constant" and list(source.get_output_shape(0)) == [shape.lin_heads, shape.hidden]:
            names.append(op.get_friendly_name())
    return names


def write_tokenizer(hf_dir: Path, out: Path) -> None:
    """The OpenVINO tokenizer (text to ids, run on the CPU) and the
    `tokenizer.json` the runner decodes ids with."""
    import shutil

    import openvino as ov
    from openvino_tokenizers import convert_tokenizer
    from transformers import AutoTokenizer

    ov.save_model(convert_tokenizer(AutoTokenizer.from_pretrained(hf_dir)), out / "openvino_tokenizer.xml")
    for name in ("tokenizer.json", "tokenizer_config.json", "LICENSE"):
        if (hf_dir / name).is_file():
            shutil.copy2(hf_dir / name, out / name)


def run(hf_dir: Path, pack_dir: Path, npu_dir: Path, device: str, cases: Path) -> int:
    import openvino as ov
    from transformers import AutoTokenizer

    shape, _ = load(hf_dir)
    tokenizer = AutoTokenizer.from_pretrained(hf_dir)
    system, prefix = system_prompt(pack_dir)
    core = ov.Core()
    config = {"CACHE_DIR": str(Path.home() / ".cache/narrivy/openvino-cache")}
    if device == "NPU":
        # The runtime's own compiler: the driver's can be older than it.
        config["NPU_COMPILER_TYPE"] = "PLUGIN"
    runner = Bound(core, npu_dir, device, config)
    stop = stop_tokens(tokenizer)
    head, _ = prompt_parts(tokenizer, system, "x")
    started = time.time()
    runner.clear()
    runner.feed(head)
    base = runner.snapshot()
    print(f"system prompt ({len(head)} tokens): {(time.time() - started) * 1000:.0f} ms")
    passed = total = 0
    for line in cases.read_text().splitlines():
        if not line.strip():
            continue
        case = json.loads(line)
        _, rest = prompt_parts(tokenizer, system, prefix + case["input"])
        started = time.time()
        runner.restore(base)
        out = runner.generate(rest, stop, 400, source=tokenizer(case["input"], add_special_tokens=False)["input_ids"])
        text = tokenizer.decode(out, skip_special_tokens=True).strip()
        ok = text == case["expected"].strip()
        passed += ok
        total += 1
        print(
            f"{'PASS' if ok else 'FAIL'} {(time.time() - started) * 1000:5.0f} ms "
            f"({len(rest)} in, {len(out)} out, {runner.calls} calls) {case['id']} [{case['category']}]"
        )
        if not ok:
            print(f"   out: {text!r}\n   exp: {case['expected']!r}")
    print(f"{passed}/{total} exact")
    return 0


def main() -> int:
    args = sys.argv[1:]
    if args[:1] == ["check"] and len(args) == 3:
        return check(Path(args[1]).expanduser(), Path(args[2]).expanduser())
    global CONTEXT
    if "--context" in args:
        at = args.index("--context")
        CONTEXT = int(args[at + 1])
        del args[at : at + 2]
    if args[:1] == ["export"] and len(args) >= 3:
        flags = args[3:]
        return export(
            Path(args[1]).expanduser(),
            Path(args[2]).expanduser(),
            "--with-step" in flags,
            "--int4" in flags,
            "--mtp" in flags,
        )
    if args[:1] == ["run"] and len(args) == 6:
        return run(*(Path(a).expanduser() for a in args[1:4]), args[4], Path(args[5]).expanduser())
    print(
        "usage:\n  qwen35_npu.py check HF_DIR PACK_DIR\n  qwen35_npu.py export HF_DIR OUT_DIR [--with-step] [--int4] [--context N]\n"
        "  qwen35_npu.py run HF_DIR PACK_DIR NPU_DIR DEVICE CASES",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
