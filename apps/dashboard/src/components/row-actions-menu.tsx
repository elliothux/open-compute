import { Button } from "@cloudflare/kumo/components/button";
import { DropdownMenu } from "@cloudflare/kumo/components/dropdown";
import { IconDots } from "@tabler/icons-react";

type RowAction = {
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
          icon={<IconDots size={16} strokeWidth={2.5} />}
        />
      </DropdownMenu.Trigger>
      <DropdownMenu.Content>
        {actions.map((action) => (
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
