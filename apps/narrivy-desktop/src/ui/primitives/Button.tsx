import type { ButtonHTMLAttributes } from 'react';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'default' | 'primary' | 'ghost';
}

export function Button({ variant = 'default', type, className, ...rest }: ButtonProps) {
  const merged = ['btn', `btn-${variant}`, className].filter(Boolean).join(' ');
  return <button type={type ?? 'button'} className={merged} {...rest} />;
}
