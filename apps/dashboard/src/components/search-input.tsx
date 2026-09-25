import { InputGroup } from "@cloudflare/kumo/components/input-group";
import { IconSearch } from "@tabler/icons-react";
import type { ComponentPropsWithoutRef } from "react";

type SearchInputProps = Omit<
  ComponentPropsWithoutRef<typeof InputGroup.Input>,
  "className" | "disabled" | "size"
> & {
  className?: string;
  disabled?: boolean;
};

/** Text input with a leading search icon, for filter and search boxes. */
export function SearchInput({
  className,
  disabled = false,
  placeholder,
  ...props
}: SearchInputProps) {
  return (
    <InputGroup className={className} disabled={disabled}>
      <InputGroup.Addon>
        <IconSearch size={16} />
      </InputGroup.Addon>
      <InputGroup.Input
        {...props}
        placeholder={placeholder}
        aria-label={props["aria-label"] ?? placeholder}
      />
    </InputGroup>
  );
}
