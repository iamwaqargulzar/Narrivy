import type { ReactNode } from 'react';

export function Card({ title, children }: { title?: ReactNode; children: ReactNode }) {
  return (
    <section className="card">
      {title ? <h2 className="card-title">{title}</h2> : null}
      {children}
    </section>
  );
}
