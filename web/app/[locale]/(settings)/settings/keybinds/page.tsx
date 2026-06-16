import { PageHeader } from "@/components/admin/PageHeader";
import { KeybindEditor } from "@/components/settings/KeybindEditor";

export default function KeybindsPage() {
  return (
    <>
      <PageHeader
        title="Keyboard shortcuts"
        description="Re-bindable hotkeys for the reader. Spacebar always advances and is not user-rebindable."
      />
      <KeybindEditor />
    </>
  );
}
