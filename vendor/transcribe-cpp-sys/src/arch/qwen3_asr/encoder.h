// arch/qwen3_asr/encoder.h - Qwen3-ASR audio encoder graph builder.
//
// The encoder is a bidirectional 18-layer transformer on top of a 3x
// Conv2d subsampler. Modeled directly off
// qwen_asr.core.transformers_backend.modeling_qwen3_asr
// .Qwen3ASRAudioEncoder.
//
// Attention: we match the reference's eager full-bidirectional attention over
// the pad-trimmed sequence (full, unmasked attention), not vLLM's
// cu_seqlens chunking.
//
// The conv subsampler and the transformer are two graphs: the subsampler
// (build_subsample_graph) runs over up to kConvPassChunks chunks at a time,
// and its outputs are concatenated host-side into the encoder graph's input.
//
// Shape conventions (ggml fast-to-slow ne[]):
//
//   mel_in      : [mel_per_chunk,          n_mels,       1, n]
//                 subsample-graph input, n <= kConvPassChunks chunks.
//                 Chunks with fewer real mel frames than mel_per_chunk
//                 are zero-padded by the caller.
//   after 3x conv: [F_ds=n_mels/8, T_ds=mel_per_chunk/8, downsample_hidden, n]
//                 (ggml conv_2d convention swaps PyTorch's H/W, which is
//                 a no-op for the square kernel + symmetric stride.)
//   subsample_in: [d_model, per_chunk_aftercnn, n_chunks]
//                 conv_out output, the encoder graph's input; PE added there.
//   after pad-select: [d_model, T_enc]  where T_enc =
//                 (n_chunks-1) * per_chunk_aftercnn + last_chunk_aftercnn.
//                 The last chunk's aftercnn trailing pad rows are dropped
//                 in the graph (matches reference's
//                 `padded_embed[padded_mask_after_cnn]` selection).
//   attn mask   : none. The single-shot encoder attends unmasked over the
//                 whole (pad-trimmed) sequence, which is what an all-zero
//                 [T_enc, T_enc] additive mask means, so the graph passes
//                 mask=nullptr rather than materialising a T^2 buffer.
//                 (The reference's measured eager path ignores cu_seqlens and
//                 runs plain bidirectional attention over the valid aftercnn
//                 rows; vLLM's flash-attn-2 windowing scores worse WER.)
//   output      : [output_dim, T_enc]

#pragma once

#include "ggml.h"
#include "weights.h"

#include <vector>

struct ggml_context;
struct ggml_cgraph;
struct ggml_tensor;

namespace transcribe::qwen3_asr {

// Per-utterance timing metadata. Computed host-side from the mel length
// and encoder hparams; drives graph shape and mask construction.
struct EncoderTiming {
    int32_t n_mel_frames        = 0;  // input mel frame count
    int32_t mel_per_chunk       = 0;  // n_window * 2
    int32_t n_chunks            = 0;  // ceil(n_mel_frames / mel_per_chunk)
    int32_t last_chunk_real_mel = 0;  // real (unpadded) mel count in the last chunk
    int32_t per_chunk_aftercnn  = 0;  // aftercnn(mel_per_chunk) — same for every full chunk
    int32_t last_chunk_aftercnn = 0;  // aftercnn(last_chunk_real_mel)
    int32_t T_enc_padded        = 0;  // n_chunks * per_chunk_aftercnn (graph sequence length)
    int32_t T_enc               = 0;  // (n_chunks-1)*per + last_after (real, ragged)
    int32_t aftercnn_lens_total = 0;  // aftercnn(n_mel_frames) — matches T_enc for single-utterance inputs
};

// Apply the 3x stride-2 pad-1 kernel-3 downsampling formula three times.
int32_t aftercnn_len(int32_t mel_len);

EncoderTiming compute_encoder_timing(int32_t n_mel_frames, const QwenAsrHParams & hp);

// Sinusoidal position table, row-major "[length, d_model]" layout.
// Matches SinusoidsPositionEmbedding: the first d_model/2 channels are
// sin(p * inv_ts[k]), the second d_model/2 are cos(p * inv_ts[k]).
std::vector<float> build_sinusoid_pe(int32_t d_model, int32_t length, double max_timescale = 10000.0);

// Conv chunks per group inside a subsample graph. The 3x Conv2d frontend is
// independent per n_window*2-mel chunk, but its activations are large:
// conv0's output is [50, 64, 480] f32 (6.1 MB) per chunk and conv1's im2col
// another 6.9 MB, ~1.2 GB of scheduler scratch per minute of audio if every
// chunk runs at once. The graph runs the chunks in groups chained by concat,
// so each group's intermediates retire before the next group's are
// allocated and scratch stays at one group's worth. The reference does the
// same with conv_chunksize (500); ours is smaller because the scheduler
// commits the whole group at once. Per-chunk results do not depend on the
// group size.
constexpr int kConvChunkGroup = 16;

// Conv chunks per subsample graph run (~8.5 minutes of audio). Each group
// adds ~36 graph nodes, so one graph over the whole input would overflow
// its node budget on long audio; longer inputs re-run a graph of this many
// chunks per pass instead. Large enough that typical inputs take one pass,
// since each pass costs a backend sync.
constexpr int kConvPassChunks = 512;

struct SubsampleBuild {
    ggml_tensor * mel_in = nullptr;  // [mel_per_chunk, n_mels, 1, n]
    ggml_tensor * out    = nullptr;  // [d_model, per_chunk_aftercnn, n]
    ggml_cgraph * graph  = nullptr;
};

// Conv subsample + conv_out linear (no PE) over n chunks, in groups of
// kConvChunkGroup.
SubsampleBuild build_subsample_graph(ggml_context *         ctx,
                                     const QwenAsrWeights & weights,
                                     const QwenAsrHParams & hp,
                                     int                    n_chunks);

struct EncoderDumps {
    ggml_tensor * subsample_out  = nullptr;  // post conv_out linear, pre-PE (graph input)
    ggml_tensor * pos_add_out    = nullptr;  // post-PE, flattened and pad-trimmed to [d_model, T_enc]
    ggml_tensor * block_0_out    = nullptr;
    ggml_tensor * block_last_out = nullptr;
    ggml_tensor * ln_post_out    = nullptr;
    ggml_tensor * proj_out       = nullptr;
};

struct EncoderBuild {
    ggml_tensor * subsample_in = nullptr;  // [d_model, per_chunk_aftercnn, n_chunks]
    ggml_tensor * pos_emb_in   = nullptr;  // [d_model, per_chunk_aftercnn]
    ggml_tensor * out          = nullptr;  // [output_dim, T_enc]
    EncoderDumps  dumps{};
    ggml_cgraph * graph = nullptr;
    EncoderTiming timing{};
};

EncoderBuild build_encoder_graph(ggml_context *         ctx,
                                 const QwenAsrWeights & weights,
                                 const QwenAsrHParams & hp,
                                 const EncoderTiming &  timing,
                                 bool                   use_flash = false);

// ---------------------------------------------------------------------------
// Batched encoder (offline transcribe_run_batch)
// ---------------------------------------------------------------------------

struct EncoderBuildBatched {
    ggml_tensor * subsample_in = nullptr;  // [d_model, per_chunk_aftercnn, B*n_chunks_max]
    ggml_tensor * pos_emb_in   = nullptr;  // [d_model, per_chunk_aftercnn]
    ggml_tensor * mask_in      = nullptr;  // [T_pad_max, T_pad_max, 1, B] key-pad
    ggml_tensor * out          = nullptr;  // [output_dim, T_pad_max, B]
    ggml_cgraph * graph        = nullptr;

    int n_batch      = 0;
    int n_chunks_max = 0;
    int T_per_chunk  = 0;
    int T_pad_max    = 0;  // n_chunks_max * per_chunk_aftercnn
};

// Build one encoder graph that processes B utterances in parallel on the
// batch axis ne[2]. All utterances share the same per_chunk_aftercnn (it
// depends only on enc_n_window), so they pack cleanly: utterance b's chunks
// occupy N-indices [b*n_chunks_max, b*n_chunks_max + n_chunks[b]) of subsample_in,
// zero-padded to n_chunks_max. The conv subsampler is per-chunk (no cross-
// utterance leak); the 18 blocks attend per-utterance under a key-pad mask
// (`mask_in[k,q,0,b] = 0 if k < T_enc[b] else -inf`). The real rows of
// utterance b are the first T_enc[b] rows of its [T_pad_max] section, so the
// caller slices out[:, 0:T_enc[b], b]. Real-row outputs are bit-identical to
// the single-shot encoder (same per-chunk conv, same masked attention).
//
// Pass use_flash=false to match the single-shot non-flash default exactly.
EncoderBuildBatched build_encoder_graph_batched(ggml_context *         ctx,
                                                const QwenAsrWeights & weights,
                                                const QwenAsrHParams & hp,
                                                int                    n_chunks_max,
                                                int                    n_batch,
                                                bool                   use_flash = false);

}  // namespace transcribe::qwen3_asr
