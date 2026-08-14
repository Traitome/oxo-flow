// Bilingual glossary tooltip for core workflow terms (issue #82 P1-8):
// renders the term as an <abbr> with a dotted underline; hovering shows
// the definition in the active language. Screen readers get the title
// attribute as well.

import { useI18n } from '../context/I18n';
import { GLOSSARY } from '../context/glossary-data';

interface GlossaryProps {
  term: string;
  children?: React.ReactNode;
}

export default function Glossary({ term, children }: GlossaryProps) {
  const { lang } = useI18n();
  const entry = GLOSSARY[term];
  if (!entry) return <>{children ?? term}</>;
  const definition = lang === 'zh' ? entry.zh : entry.en;
  return (
    <abbr
      title={definition}
      style={{
        textDecoration: 'underline dotted',
        textUnderlineOffset: '3px',
        cursor: 'help',
      }}
    >
      {children ?? term}
    </abbr>
  );
}
