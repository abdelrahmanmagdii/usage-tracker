/** The profile a reading comes from and its masked identifier, when known. */
export function AccountChip({
  sourceLabel,
  maskedIdentity,
}: {
  sourceLabel?: string;
  maskedIdentity?: string;
}) {
  if (!sourceLabel && !maskedIdentity) return null;
  const visible = [sourceLabel, maskedIdentity].filter(Boolean).join(" · ");
  const full = [sourceLabel && `Profile: ${sourceLabel}`, maskedIdentity && `Account: ${maskedIdentity}`]
    .filter(Boolean)
    .join("\n");
  return (
    <span className="account-chip" title={full}>
      {visible}
    </span>
  );
}
