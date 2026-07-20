import { ArrowUpRight, Layers3, Sparkles } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"

const capabilities = [
  { icon: Sparkles, title: "Visual direction", copy: "Shape the interface from language, references, and design tokens." },
  { icon: Layers3, title: "Editable components", copy: "Build with local React and shadcn component source you can keep refining." },
]

export default function Page() {
  return (
    <main className="mx-auto flex min-h-svh max-w-6xl flex-col px-[var(--spacing-page-gutter)] py-8">
      <nav className="flex items-center justify-between border-b py-4">
        <span className="text-sm font-semibold tracking-tight">AnyDesign</span>
        <Button variant="ghost" size="sm">Preview ready</Button>
      </nav>
      <section className="grid flex-1 items-center gap-12 py-[var(--spacing-section)] lg:grid-cols-[1.25fr_0.75fr]">
        <div className="max-w-3xl">
          <p className="mb-5 text-sm font-medium text-primary">React canvas · ready to shape</p>
          <h1 className="text-balance text-5xl font-semibold tracking-[-0.045em] sm:text-7xl">
            Turn a clear idea into a distinctive interface.
          </h1>
          <p className="mt-6 max-w-2xl text-pretty text-lg leading-8 text-muted-foreground">
            This Next.js foundation includes Base UI powered shadcn components, Tailwind CSS v4,
            responsive defaults, and a production static export contract.
          </p>
          <div className="mt-8 flex flex-wrap gap-3">
            <Button size="lg">Start creating <ArrowUpRight data-icon="inline-end" /></Button>
            <Button size="lg" variant="outline">Inspect components</Button>
          </div>
        </div>
        <div className="grid gap-4">
          {capabilities.map(({ icon: Icon, title, copy }) => (
            <Card key={title} className="shadow-[var(--shadow-soft)]">
              <CardHeader>
                <Icon className="mb-5 size-5 text-primary" aria-hidden="true" />
                <CardTitle>{title}</CardTitle>
                <CardDescription>{copy}</CardDescription>
              </CardHeader>
              <CardContent className="text-xs text-muted-foreground">Template contract: next-app@1</CardContent>
            </Card>
          ))}
        </div>
      </section>
    </main>
  )
}
