"use client";

import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";

import { Button } from "@/components/ui/button";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useUpdateSettings } from "@/lib/api/mutations";

import type { SmtpInitial } from "./EmailAdminClient";

const formSchema = z.object({
  host: z.string().trim(),
  port: z
    .string()
    .trim()
    .refine(
      (v) => v === "" || (/^\d+$/.test(v) && Number(v) >= 1 && Number(v) <= 65535),
      "Port must be between 1 and 65535",
    ),
  tls: z.enum(["none", "starttls", "tls"]),
  username: z.string(),
  /** Empty string means "no change" when a password is already set. */
  password: z.string(),
  from: z.string().trim(),
});

type FormValues = z.infer<typeof formSchema>;

export function EmailConfigForm({ initial }: { initial: SmtpInitial }) {
  const update = useUpdateSettings();

  const form = useForm<FormValues>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      host: initial.host,
      port: initial.port,
      tls: initial.tls,
      username: initial.username,
      password: "",
      from: initial.from,
    },
  });

  async function onSubmit(values: FormValues) {
    // The submit button binds to `formState.isDirty` so the no-op path
    // can't reach this handler. We still build the patch from a per-key
    // diff to keep the audit log signal-rich (only fields the user
    // actually changed).
    const patch: Record<string, unknown> = {};

    // Strings: send `null` to clear, the trimmed value to set.
    const diffString = (k: keyof SmtpInitial, key: string, next: string) => {
      const prev = String(initial[k] ?? "");
      if (prev === next) return;
      patch[key] = next === "" ? null : next;
    };
    diffString("host", "smtp.host", values.host.trim());
    diffString("username", "smtp.username", values.username);
    diffString("from", "smtp.from", values.from.trim());

    if (values.port !== initial.port) {
      patch["smtp.port"] = values.port === "" ? null : Number(values.port);
    }
    if (values.tls !== initial.tls) {
      patch["smtp.tls"] = values.tls;
    }

    // Password: empty input keeps the existing row untouched.
    if (values.password !== "") {
      patch["smtp.password"] = values.password;
    }

    await update.mutateAsync(patch);
  }

  return (
    <Form {...form}>
      <form
        onSubmit={form.handleSubmit(onSubmit)}
        className="space-y-4"
        noValidate
      >
        <FormField
          control={form.control}
          name="host"
          render={({ field }) => (
            <FormItem>
              <FormLabel>SMTP host</FormLabel>
              <FormControl>
                <Input placeholder="smtp.example.com" {...field} />
              </FormControl>
              <FormDescription>
                Leave blank to disable email delivery (recovery flows fall back
                to logging links to the server log).
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />

        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <FormField
            control={form.control}
            name="port"
            render={({ field }) => (
              <FormItem>
                <FormLabel>Port</FormLabel>
                <FormControl>
                  <Input
                    inputMode="numeric"
                    placeholder="587"
                    {...field}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name="tls"
            render={({ field }) => (
              <FormItem>
                <FormLabel>TLS</FormLabel>
                <Select onValueChange={field.onChange} value={field.value}>
                  <FormControl>
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent>
                    <SelectItem value="starttls">STARTTLS</SelectItem>
                    <SelectItem value="tls">Implicit TLS</SelectItem>
                    <SelectItem value="none">None (cleartext)</SelectItem>
                  </SelectContent>
                </Select>
                <FormDescription>
                  Most providers use STARTTLS on 587 or implicit TLS on 465.
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>

        <FormField
          control={form.control}
          name="username"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Username</FormLabel>
              <FormControl>
                <Input autoComplete="off" {...field} />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="password"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Password</FormLabel>
              <FormControl>
                <Input
                  type="password"
                  autoComplete="new-password"
                  placeholder={initial.password_set ? "•••••••• (unchanged)" : ""}
                  {...field}
                />
              </FormControl>
              <FormDescription>
                {initial.password_set
                  ? "A password is stored. Leave blank to keep it."
                  : "Stored encrypted at rest. Never echoed back over the API."}
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="from"
          render={({ field }) => (
            <FormItem>
              <FormLabel>From address</FormLabel>
              <FormControl>
                <Input
                  type="email"
                  placeholder="noreply@example.com"
                  {...field}
                />
              </FormControl>
              <FormDescription>
                Plain address only — display names with angle brackets confuse
                some MTAs and the dev <code>just</code> dotenv parser.
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />

        <div className="flex justify-end">
          <Button
            type="submit"
            disabled={!form.formState.isDirty || update.isPending}
          >
            {update.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </form>
    </Form>
  );
}
