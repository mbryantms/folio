import { redirect } from "next/navigation";

export default async function SettingsIndexPage() {
  redirect(`/settings/reading`);
}
