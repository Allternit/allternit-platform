/**
 * Allternit Extension — Password Autofill Content Script
 *
 * Detects login forms and requests the background to autofill credentials
 * from the Allternit vault. The content script never sees plaintext passwords;
 * it only reports field presence and receives fill instructions.
 */

const DEBUG_PREFIX = '[Allternit Autofill]';

export default defineContentScript({
  matches: ['<all_urls>'],
  runAt: 'document_end',

  main() {
    console.debug(`${DEBUG_PREFIX} Loaded on ${window.location.href}`);

    // Wait for the page to settle before scanning.
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', () => scanAndAutofill());
    } else {
      scanAndAutofill();
    }

    // Re-scan when the DOM changes significantly (SPA navigation, dynamic forms).
    let debounceTimer: number | null = null;
    const observer = new MutationObserver(() => {
      if (debounceTimer) window.clearTimeout(debounceTimer);
      debounceTimer = window.setTimeout(() => scanAndAutofill(), 500);
    });
    observer.observe(document.body, { childList: true, subtree: true });

    // Listen for fill instructions from the background.
    chrome.runtime.onMessage.addListener((message, _sender, sendResponse): true | undefined => {
      if (message?.type === 'AUTOFILL_FILL_FIELDS') {
        handleFillInstruction(message.payload)
          .then((result) => sendResponse({ ok: true, result }))
          .catch((error) => sendResponse({ ok: false, error: error instanceof Error ? error.message : String(error) }));
        return true;
      }
      if (message?.type === 'AUTOFILL_RESCAN') {
        // The person approved filling on this page (agent pause).
        lastAutofilledHost = null;
        scanAndAutofill();
        sendResponse({ ok: true });
        return undefined;
      }
      return undefined;
    });
  },
});

interface LoginFormFields {
  usernameField: HTMLInputElement | null;
  passwordField: HTMLInputElement | null;
  submitButton: HTMLElement | null;
  form: HTMLFormElement | null;
}

let lastAutofilledHost: string | null = null;

function scanAndAutofill(): void {
  const origin = normalizeOrigin(window.location.href);
  if (lastAutofilledHost === origin) return;

  const fields = findLoginFormFields();
  if (!fields.usernameField && !fields.passwordField) return;
  if (fields.usernameField?.value || fields.passwordField?.value) return;

  console.debug(`${DEBUG_PREFIX} Login form detected on ${origin}`);
  chrome.runtime
    .sendMessage({
      type: 'AUTOFILL_REQUEST_CREDENTIALS',
      payload: { origin, url: window.location.href },
    })
    .then((response) => {
      if (response?.ok && response.credentials && response.credentials.length > 0) {
        const best = response.credentials[0];
        return chrome.runtime.sendMessage({
          type: 'AUTOFILL_FILL_CREDENTIAL',
          payload: { credentialId: best.id, origin },
        });
      }
    })
    .catch((error) => {
      console.error(`${DEBUG_PREFIX} Failed to request credentials:`, error);
    });
}

function findLoginFormFields(): LoginFormFields {
  const passwordField = findPasswordField();
  const usernameField = passwordField
    ? findUsernameField(passwordField)
    : findStandaloneUsernameField();
  const form = passwordField?.closest('form') || usernameField?.closest('form') || null;
  const submitButton = form ? findSubmitButton(form) : null;
  return { usernameField, passwordField, submitButton, form };
}

function findPasswordField(): HTMLInputElement | null {
  const inputs = Array.from(document.querySelectorAll('input'));
  return (
    inputs.find((input) => input.type === 'password' && isVisible(input)) || null
  );
}

function findUsernameField(passwordField: HTMLInputElement): HTMLInputElement | null {
  const inputs = Array.from(document.querySelectorAll('input'));
  const passwordIndex = inputs.indexOf(passwordField);

  // Search backwards from the password field for the most likely username input.
  for (let i = passwordIndex - 1; i >= 0; i--) {
    const input = inputs[i];
    if (isUsernameInput(input) && isVisible(input)) {
      return input;
    }
  }

  // Fallback: search the same form.
  const form = passwordField.closest('form');
  if (form) {
    const formInputs = Array.from(form.querySelectorAll('input'));
    return (
      formInputs.find((input) => isUsernameInput(input) && isVisible(input)) || null
    );
  }

  return null;
}

function findStandaloneUsernameField(): HTMLInputElement | null {
  const inputs = Array.from(document.querySelectorAll('input'));
  return inputs.find((input) => isUsernameInput(input) && isVisible(input)) || null;
}

function isUsernameInput(input: HTMLInputElement): boolean {
  if (input.type === 'password') return false;
  const autocomplete = (input.getAttribute('autocomplete') || '').toLowerCase();
  if (autocomplete.includes('username') || autocomplete.includes('email')) return true;
  const name = (input.name || '').toLowerCase();
  const id = (input.id || '').toLowerCase();
  const placeholder = (input.placeholder || '').toLowerCase();
  const usernameHints = ['user', 'email', 'login', 'account', 'name'];
  return usernameHints.some(
    (hint) => name.includes(hint) || id.includes(hint) || placeholder.includes(hint),
  );
}

function findSubmitButton(form: HTMLFormElement): HTMLElement | null {
  const submit = form.querySelector('button[type="submit"], input[type="submit"]') as HTMLElement | null;
  if (submit) return submit;
  const buttons = Array.from(form.querySelectorAll('button'));
  return buttons.find((btn) => /sign.in|log.in|submit|continue|next/i.test(btn.innerText)) || null;
}

function isVisible(element: HTMLElement): boolean {
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
}

function normalizeOrigin(url: string): string {
  try {
    const { hostname } = new URL(url);
    return hostname.toLowerCase();
  } catch {
    return url.toLowerCase();
  }
}

async function handleFillInstruction(payload: {
  username: string;
  password: string;
  credentialId: string;
}): Promise<{ filled: boolean }> {
  const { username, password } = payload;
  const filled = fillFields(username, password);
  if (filled) {
    lastAutofilledHost = normalizeOrigin(window.location.href);
  }
  return { filled };
}

function fillFields(username: string | null | undefined, password: string | null | undefined): boolean {
  const fields = findLoginFormFields();
  let filled = false;

  if (fields.usernameField && username) {
    setInputValue(fields.usernameField, username);
    filled = true;
  }

  if (fields.passwordField && password) {
    setInputValue(fields.passwordField, password);
    filled = true;
  }

  if (filled) {
    console.debug(`${DEBUG_PREFIX} Filled login form`);
  }

  return filled;
}

function setInputValue(input: HTMLInputElement, value: string): void {
  input.focus();
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new Event('change', { bubbles: true }));
  input.dispatchEvent(new Event('blur', { bubbles: true }));
}
