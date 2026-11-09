export const ONBOARDED_STORAGE_KEY = 'narrivy.onboarded.v1';

export function hasOnboarded(): boolean {
  try {
    return window.localStorage.getItem(ONBOARDED_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

export function markOnboarded(): void {
  try {
    window.localStorage.setItem(ONBOARDED_STORAGE_KEY, '1');
  } catch {
    // storage unavailable; onboarding will repeat next launch
  }
}
