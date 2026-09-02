interface DetailTab {
  id: string;
  label: string;
}

interface DetailTabsProps {
  tabs: DetailTab[];
  activeTab: string;
  onTabChange: (tabId: string) => void;
}

export function DetailTabs({ tabs, activeTab, onTabChange }: DetailTabsProps) {
  return (
    <Tabs
      className="mb-6"
      variant="underline"
      tabs={tabs.map(tab => ({ value: tab.id, label: tab.label }))}
      value={activeTab}
      onValueChange={onTabChange}
    />
  );
}
import { Tabs } from "@cloudflare/kumo/components/tabs";
