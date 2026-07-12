export function Button({ children, className = '', ...props }) {
  return (
    <button className={`runtime-button px-4 py-2 font-semibold ${className}`} {...props}>
      {children}
    </button>
  );
}
