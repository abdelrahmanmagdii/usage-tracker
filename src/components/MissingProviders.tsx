import { useEffect, useState } from "react";
import type { MissingProvider } from "../lib/providers";

export function MissingProviders({ items }: { items: MissingProvider[] }) {
  const [open, setOpen] = useState(false);
  const hasItems = items.length > 0;
  useEffect(() => {
    if (hasItems) setOpen(true);
  }, [hasItems]);

  return (
    <details className="missing-providers" open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>Don't see your provider?</summary>
      <p>Here is what is missing, and how to add it.</p>
      {items.length ? (
        <ul>
          {items.map((item) => (
            <li key={item.id}>
              <strong>{item.label}</strong>
              <span>{item.detail}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="missing-providers-clear">Every supported tool is already in the list above.</p>
      )}
    </details>
  );
}
