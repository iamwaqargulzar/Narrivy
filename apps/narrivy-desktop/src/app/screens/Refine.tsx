// Refine: what happens to the text after it is transcribed and before it is
// inserted (owner decision D23, ADR-0018). Two parts that stay separate:
// deterministic rules, which always run on this computer, and an optional
// language model. This first version is read-only: it states what the rules
// do today and shows the scheduled options with their task, never as working.
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { useNav } from '../../state/NavContext';
import { usePipelines } from '../../state/PipelinesContext';
import { useDaemonStatus } from '../../state/StatusContext';
import { recognitionOptions } from '../../lib/recognition';
import { Badge, Choices, LIVE, Option, REPORTED, planned } from '../components/StandingOptions';

/** The deterministic corrector's rules, in the order it applies them. */
const RULES = ['spacing', 'punctuation', 'capitals', 'ending', 'numbers'] as const;

export function RefineScreen() {
  const { navigate } = useNav();
  const { catalog } = usePipelines();
  const { status } = useDaemonStatus();
  const selected = catalog.pipelines.find((p) => p.id === catalog.selectedId) ?? null;
  // The rules run in the Clean pipeline and are skipped in Raw.
  const rulesOn = selected?.id === 'clean';
  const rulesValue = selected === null ? t('home.status.notReported') : rulesOn ? t('pipe.on') : t('pipe.off');
  const hints = recognitionOptions(status).contextBias;

  return (
    <div className="page-wide">
      <div className="page-head">
        <h2 className="page-title">{t('nav.refine')}</h2>
        <p className="page-lead">{t('refine.lead')}</p>
      </div>

      <section className="pipe-card refine-card" aria-labelledby="refine-rules-title">
        <div className="refine-head">
          <span className="stage-icon" aria-hidden="true">
            <Icon name="sliders" />
          </span>
          <div>
            <h3 id="refine-rules-title">{t('refine.rules.title')}</h3>
            <p className="pipe-option-hint">{t('refine.rules.lead')}</p>
          </div>
          <span className="card-head-spacer" />
          <span className={`chip${rulesOn ? ' chip-ok' : ''}`}>
            {selected ? t('refine.rules.inPipeline', { state: rulesValue, name: selected.name }) : rulesValue}
          </span>
        </div>
        <div className="pipe-panel refine-panel">
          {RULES.map((rule) => (
            <Option
              key={rule}
              label={t(`refine.rule.${rule}`)}
              hint={t(`refine.rule.${rule}Hint`)}
              value={rulesValue}
              standing={REPORTED}
            />
          ))}
          <Option label={t('refine.rules.perRule')} hint={t('refine.rules.perRuleHint')} standing={planned('T2.20')} />
          <Option label={t('pipe.refine.vocab')} hint={t('pipe.refine.vocabHint')} standing={planned('T2.14')} />
          <Option
            label={t('pipe.refine.hints')}
            hint={t('refine.hintsHint')}
            value={hints ? t('refine.available') : t('refine.notForModel')}
            standing={hints ? LIVE : REPORTED}
          >
            <button type="button" className="link-action link-quiet" onClick={() => navigate('pipelines')}>
              {t('refine.openPipelines')}
              <Icon name="arrowRight" />
            </button>
          </Option>
        </div>
      </section>

      <section className="pipe-card refine-card" aria-labelledby="refine-model-title">
        <div className="refine-head">
          <span className="stage-icon" aria-hidden="true">
            <Icon name="spark" />
          </span>
          <div>
            <h3 id="refine-model-title">
              {t('refine.model.title')}
              <Badge standing={planned('T2.20')} />
            </h3>
            <p className="pipe-option-hint">{t('refine.model.lead')}</p>
          </div>
        </div>
        <div className="pipe-panel refine-panel">
          <Option label={t('refine.model.installed')} hint={t('refine.model.installedHint')} value={t('refine.model.none')} standing={planned('T2.19')} />
          <Option label={t('pipe.model.accel')} hint={t('refine.model.whereHint')} standing={planned('T2.20')}>
            <Choices
              current="auto"
              options={[
                { id: 'auto', label: t('pipe.model.auto') },
                { id: 'npu', label: 'NPU' },
                { id: 'gpu', label: 'GPU' },
                { id: 'cpu', label: 'CPU' },
              ]}
            />
          </Option>
          <Option label={t('refine.level.title')} hint={t('refine.level.hint')} standing={planned('T2.20')}>
            <Choices
              current="off"
              options={[
                { id: 'off', label: t('pipe.off') },
                { id: 'faithful', label: t('refine.level.faithful'), note: t('refine.level.faithfulNote') },
                { id: 'polish', label: t('refine.level.polish'), note: t('refine.level.polishNote') },
                { id: 'rewrite', label: t('refine.level.rewrite'), note: t('refine.level.rewriteNote') },
              ]}
            />
          </Option>
          <Option label={t('pipe.refine.guard')} hint={t('refine.guardHint')} standing={planned('T2.20')} />
          <Option label={t('refine.privacy')} hint={t('refine.privacyHint')} value={t('refine.localOnly')} standing={planned('T2.20')} />
        </div>
      </section>
    </div>
  );
}
