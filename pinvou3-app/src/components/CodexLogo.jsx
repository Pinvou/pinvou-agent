import React from 'react';
import openaiIcon from '../brand-icons/openai.svg';

export function CodexLogo({ className = 'h-4 w-4', title }) {
  return (
    <span
      role={title ? 'img' : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      className={`inline-block shrink-0 bg-current ${className}`}
      style={{
        WebkitMaskImage: `url("${openaiIcon}")`,
        maskImage: `url("${openaiIcon}")`,
        WebkitMaskPosition: 'center',
        maskPosition: 'center',
        WebkitMaskRepeat: 'no-repeat',
        maskRepeat: 'no-repeat',
        WebkitMaskSize: 'contain',
        maskSize: 'contain',
      }}
    />
  );
}
