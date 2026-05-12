import { ResetPasswordForm } from "./ResetPasswordForm";

type SearchParams = {
  token?: string;
};

export default async function ResetPasswordPage({
  searchParams,
}: {
  searchParams: Promise<SearchParams>;
}) {
  const sp = await searchParams;
  return (
    <div className="bg-background flex min-h-screen items-center justify-center px-4 py-12">
      <div className="w-full max-w-sm">
        <ResetPasswordForm token={sp.token ?? null} />
      </div>
    </div>
  );
}
