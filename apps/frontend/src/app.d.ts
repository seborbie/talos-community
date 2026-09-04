/// <reference types="@sveltejs/kit" />

interface ImportMetaEnv {
  readonly PUBLIC_API_URL?: string;
  readonly PUBLIC_RMM_API_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
