// The Voice stage's short summary for one pipeline, shared by Home and
// Pipelines so both say the same thing.
import { t } from '../i18n';
import type { AudioOptions } from './bridge';

/** This pipeline's own Voice choices: whether the computer's sound is
 * ignored and whether only the user's voice is kept. */
export function pipelineVoice(audio: AudioOptions, pipelineId: string) {
  const own = audio.pipelines?.[pipelineId] ?? {};
  return {
    echo: own.echo_cancel ?? audio.echo_cancel,
    onlyMe:
      Boolean(audio.voice_trained) &&
      (own.only_my_voice ?? Boolean(audio.only_my_voice)),
  };
}

export function voiceSummary(audio: AudioOptions | null, pipelineId: string): string {
  if (!audio) return t('pipe.off');
  const { echo, onlyMe } = pipelineVoice(audio, pipelineId);
  return (
    [echo ? t('pipe.noise.chipEcho') : null, onlyMe ? t('pipe.noise.chipVoice') : null]
      .filter(Boolean)
      .join(' · ') || t('pipe.noise.chipNone')
  );
}
