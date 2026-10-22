// Provider composition. Order matters:
// Theme → Nav → Toast (needs Nav for recovery actions)
// → Status → Pipelines → Session → Onboarding gate → Diagnostics → Shell.
// Onboarding, Home, Dictation and the palette share one advertised-pipeline
// selection, so the providers live above the onboarding gate.
import { useState } from 'react';
import { AppShell } from './app/AppShell';
import { Onboarding } from './app/Onboarding';
import { hasOnboarded, markOnboarded } from './lib/onboarding';
import { NavProvider } from './state/NavContext';
import { DiagnosticsProvider } from './state/DiagnosticsContext';
import { PipelinesProvider } from './state/PipelinesContext';
import { SessionProvider } from './state/SessionContext';
import { AssistantProvider } from './state/AssistantContext';
import { StatusProvider } from './state/StatusContext';
import { ThemeProvider } from './theme/ThemeProvider';
import { ToastProvider } from './ui/primitives/Toast';

function OnboardingGate() {
  const [onboarded, setOnboarded] = useState(hasOnboarded);

  if (!onboarded) {
    return (
      <Onboarding
        onDone={() => {
          markOnboarded();
          setOnboarded(true);
        }}
      />
    );
  }

  return (
    <DiagnosticsProvider>
      <AppShell />
    </DiagnosticsProvider>
  );
}

export default function App() {
  return (
    <ThemeProvider>
      <NavProvider>
        <ToastProvider>
          <StatusProvider>
            <PipelinesProvider>
              <SessionProvider>
                <AssistantProvider>
                  <OnboardingGate />
                </AssistantProvider>
              </SessionProvider>
            </PipelinesProvider>
          </StatusProvider>
        </ToastProvider>
      </NavProvider>
    </ThemeProvider>
  );
}
