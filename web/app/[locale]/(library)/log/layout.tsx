import { LogChrome, LogPanel } from "@/components/log/LogChrome";

/** Reading log shell: the shared header + tabs (Overview · Finished ·
 *  Stats · Sessions) over each tab's page. Plan:
 *  ~/.claude/plans/folio-flair-phase4-log.md */
export default function ReadingLogLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-6">
      <LogChrome />
      <LogPanel>{children}</LogPanel>
    </div>
  );
}
