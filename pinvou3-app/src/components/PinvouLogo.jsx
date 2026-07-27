import React from 'react';

export function PinvouLogo({ className = 'h-4 w-4', title }) {
  return (
    <img
      src="/assets/brand/brand-blue.png"
      alt={title || ''}
      aria-hidden={title ? undefined : true}
      className={`shrink-0 object-contain ${className}`}
    />
  );
}
