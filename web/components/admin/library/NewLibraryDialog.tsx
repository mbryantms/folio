"use client";

import { useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
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
import { useCreateLibrary } from "@/lib/api/mutations";

const schema = z.object({
  name: z.string().min(1, "Name is required").max(120),
  root_path: z.string().min(1, "Root path is required"),
  default_language: z
    .string()
    .regex(/^[a-z]{2,3}$/i, "Use a 2–3 letter ISO code")
    .default("eng"),
  default_reading_direction: z.enum(["ltr", "rtl"]).default("ltr"),
  scan_now: z.boolean().default(false),
  generate_page_thumbs_on_scan: z.boolean().default(false),
});

type FormValues = z.infer<typeof schema>;

export function NewLibraryDialog() {
  const [open, setOpen] = useState(false);
  const create = useCreateLibrary();
  const form = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: {
      name: "",
      root_path: "",
      default_language: "eng",
      default_reading_direction: "ltr",
      scan_now: false,
      generate_page_thumbs_on_scan: false,
    },
  });
  const scanNow = form.watch("scan_now");

  const onSubmit = form.handleSubmit((values) => {
    create.mutate(values, {
      onSuccess: () => {
        setOpen(false);
        form.reset();
      },
    });
  });

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button>New library</Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New library</DialogTitle>
          <DialogDescription>
            Point at a folder on disk. The first scan starts manually from the
            library overview.
          </DialogDescription>
        </DialogHeader>
        <Form {...form}>
          <form onSubmit={onSubmit} className="space-y-4">
            <FormField
              control={form.control}
              name="name"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>Name</FormLabel>
                  <FormControl>
                    <Input placeholder="e.g. Main collection" {...field} />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="root_path"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>Root path</FormLabel>
                  <FormControl>
                    <Input
                      placeholder="/srv/comics"
                      className="font-mono text-sm"
                      {...field}
                    />
                  </FormControl>
                  <FormDescription>
                    Absolute path on the server, must be readable.
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
            <div className="grid grid-cols-2 gap-4">
              <FormField
                control={form.control}
                name="default_language"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Default language</FormLabel>
                    <FormControl>
                      <Input className="font-mono" {...field} />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={form.control}
                name="default_reading_direction"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Reading direction</FormLabel>
                    <FormControl>
                      <select
                        className="border-input bg-background h-9 w-full rounded-md border px-3 text-sm"
                        {...field}
                      >
                        <option value="ltr">Left to right</option>
                        <option value="rtl">Right to left</option>
                      </select>
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </div>
            <FormField
              control={form.control}
              name="scan_now"
              render={({ field }) => (
                <FormItem className="border-border flex items-start gap-3 rounded-md border p-3">
                  <FormControl>
                    <Checkbox
                      checked={field.value}
                      onCheckedChange={(v) => field.onChange(v === true)}
                    />
                  </FormControl>
                  <div className="space-y-1">
                    <FormLabel>Scan after creating</FormLabel>
                    <FormDescription>
                      Leave off to scan later from the library overview. Cover
                      thumbnails are always generated on the first scan.
                    </FormDescription>
                  </div>
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="generate_page_thumbs_on_scan"
              render={({ field }) => (
                <FormItem
                  className={
                    "border-border flex items-start gap-3 rounded-md border p-3 " +
                    (scanNow ? "" : "opacity-60")
                  }
                >
                  <FormControl>
                    <Checkbox
                      checked={field.value}
                      onCheckedChange={(v) => field.onChange(v === true)}
                    />
                  </FormControl>
                  <div className="space-y-1">
                    <FormLabel>Also generate page thumbnails</FormLabel>
                    <FormDescription>
                      Page-strip thumbnails are pricier than covers (one image
                      per page). Persisted as a library setting — every future
                      scan will keep generating them. You can change this later
                      from library settings, or fill in missing page thumbnails
                      manually from the Thumbnails tab.
                    </FormDescription>
                  </div>
                </FormItem>
              )}
            />
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={() => setOpen(false)}
                disabled={create.isPending}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={create.isPending}>
                {create.isPending ? "Creating…" : "Create"}
              </Button>
            </DialogFooter>
          </form>
        </Form>
      </DialogContent>
    </Dialog>
  );
}
