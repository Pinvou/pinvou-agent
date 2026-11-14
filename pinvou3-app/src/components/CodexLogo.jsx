import openaiIcon from '../brand-icons/openai.svg';
import { MonoLogo } from './MonoLogo.jsx';

// The Codex mark renders through the shared monochrome mask (MonoLogo); see
// its comment for the role/aria semantics.
export function CodexLogo({ className = 'h-4 w-4', title }) {
  return <MonoLogo url={openaiIcon} className={className} title={title} />;
}
