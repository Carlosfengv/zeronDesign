import { source } from '../../lib/source';
import { baseOptions } from '../../lib/layout.shared';
import { DocsLayout } from 'fumadocs-ui/layouts/docs';

export default function Layout({ children }) {
  return (
    <DocsLayout tree={source.pageTree} {...baseOptions()}>
      {children}
    </DocsLayout>
  );
}
