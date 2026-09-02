import { DotsThree } from "@phosphor-icons/react";
import { Button } from "@cloudflare/kumo/components/button";
import { DropdownMenu } from "@cloudflare/kumo/components/dropdown";

export type RowAction = {
  id: string;
  label: string;
  onSelect: () => void;
  variant?: "default" | "danger";
  disabled?: boolean;
};

interface RowActionsMenuProps {
  label: string;
  actions: RowAction[];
}

export function RowActionsMenu({ label, actions }: RowActionsMenuProps) {
  if (actions.length === 0) return null;

  return (
    <DropdownMenu>
      <DropdownMenu.Trigger>
        <Button
          variant="secondary"
          aria-label={`Actions for ${label}`}
          icon={<DotsThree size={16} weight="bold" />}
        />
      </DropdownMenu.Trigger>
      <DropdownMenu.Content>
        {actions.map(action => (
          <DropdownMenu.Item
            key={action.id}
            variant={action.variant === "danger" ? "danger" : "default"}
            disabled={action.disabled}
            onClick={action.onSelect}
          >
            {action.label}
          </DropdownMenu.Item>
        ))}
      </DropdownMenu.Content>
    </DropdownMenu>
  );
}
