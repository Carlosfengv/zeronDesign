import defaultMdxComponents from 'fumadocs-ui/mdx';
import { Accordion, Accordions } from 'fumadocs-ui/components/accordion';
import { Step, Steps as FumadocsSteps } from 'fumadocs-ui/components/steps';
import { Tab, Tabs as FumadocsTabs } from 'fumadocs-ui/components/tabs';

function Steps(props) {
  return <FumadocsSteps {...props} />;
}

Steps.Step = Step;

function Tabs(props) {
  return <FumadocsTabs {...props} />;
}

Tabs.Tab = Tab;

function CompatibleAccordions(props) {
  return <Accordions {...props} />;
}

CompatibleAccordions.Accordion = Accordion;

export function getMDXComponents(components = {}) {
  return {
    ...defaultMdxComponents,
    Accordion,
    Accordions: CompatibleAccordions,
    Step,
    Steps,
    Tab,
    Tabs,
    ...components,
  };
}

export const useMDXComponents = getMDXComponents;
