import { UserDetail } from "@/components/admin/users/UserDetail";

export default async function UserDetailPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return <UserDetail id={slug} />;
}
