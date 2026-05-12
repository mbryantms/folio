import { ForgotPasswordForm } from "./ForgotPasswordForm";

type SearchParams = {
  /** Set by the server's form-fallback 303 (`/forgot-password?sent=1`) so
   *  the page can render the "check your email" confirmation even when JS
   *  never ran on the submit. */
  sent?: string;
};

export default async function ForgotPasswordPage({
  searchParams,
}: {
  searchParams: Promise<SearchParams>;
}) {
  const sp = await searchParams;
  return (
    <div className="bg-background flex min-h-screen items-center justify-center px-4 py-12">
      <div className="w-full max-w-sm">
        <ForgotPasswordForm preSubmitted={sp.sent === "1"} />
      </div>
    </div>
  );
}
