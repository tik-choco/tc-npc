// Subscribes a component to the current UI language (lib/i18n.ts). Every
// component that renders text calls this directly rather than taking `t` as
// a prop — the store is a module-level singleton, so a language switch
// re-renders all of them at once without a context provider.
import { useCallback, useEffect, useState } from "preact/hooks";

import {
  getLang,
  setLang as setLangGlobal,
  subscribeLang,
  translate,
  type Lang,
  type MessageParams,
  type MessageKey,
  type Translate,
} from "../lib/i18n";

export interface UseI18nResult {
  lang: Lang;
  setLang(lang: Lang): void;
  t: Translate;
}

export function useI18n(): UseI18nResult {
  const [lang, setLangState] = useState<Lang>(() => getLang());

  useEffect(() => subscribeLang(() => setLangState(getLang())), []);

  // Keeps <html lang> honest for screen readers and CJK font selection.
  // index.html ships lang="ja"; this corrects it once the choice is known.
  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  const t = useCallback(
    (key: MessageKey, params?: MessageParams) => translate(lang, key, params),
    [lang],
  );

  return { lang, setLang: setLangGlobal, t };
}
