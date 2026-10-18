/** Keep non-login fields out of browser and password-manager autofill. */
export const nonCredentialAutofill = {
  autoComplete: "off",
  "data-1p-ignore": "true",
  "data-op-ignore": "true",
  "data-lpignore": "true",
  "data-bwignore": "true",
  "data-form-type": "other",
} as const;
