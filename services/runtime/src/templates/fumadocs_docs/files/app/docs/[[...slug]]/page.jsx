import { notFound } from 'next/navigation';
import { source } from '../../../lib/source';
import { getMDXComponents } from '../../../components/mdx';
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from 'fumadocs-ui/layouts/docs/page';

export function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata({ params }) {
  const resolved = await params;
  const page = source.getPage(resolved.slug);
  if (!page) return { title: 'AnyDesign Runtime Docs' };
  return { title: page.data.title, description: page.data.description };
}

export default async function Page({ params }) {
  const resolved = await params;
  const page = source.getPage(resolved.slug);
  if (!page) notFound();
  const MDXContent = page.data.body;
  return (
    <DocsPage toc={page.data.toc} full={page.data.full}>
      <DocsTitle>{page.data.title}</DocsTitle>
      <DocsDescription>{page.data.description}</DocsDescription>
      <DocsBody>
        <MDXContent components={getMDXComponents()} />
      </DocsBody>
    </DocsPage>
  );
}
