/* Optional C++ convenience layer over the wickra-exchange C ABI.
 *
 * Hand-written. Unlike `wickra_exchange.h`, which cbindgen regenerates from the
 * Rust source, nothing generates this file -- editing it is the intended way to
 * change it.
 *
 * The C ABI hands out five kinds of opaque handle, each of which must be
 * released exactly once with its matching `wickra_*_free`. Every early return
 * between the constructor and that call leaks the handle, and an exception
 * thrown in between leaks it unconditionally. `wickra::Handle` wraps the pair in
 * a move-only RAII owner, so the free happens at scope exit however the scope is
 * left:
 *
 *     #include "wickra_exchange.hpp"
 *
 *     const char *assets[] = {"USDT"};
 *     const double amounts[] = {100000.0};
 *
 *     wickra::Exchange ex(wickra_paper_new(assets, amounts, 1, 1.0, 5.0, 10.0));
 *     if (!ex) {
 *         return 1;                      // nothing to free
 *     }
 *     wickra_exchange_set_price(ex.get(), "BTC/USDT", 20000.0);
 *     // wickra_exchange_free(ex.get()) happens here
 *
 * Header-only, and adds no runtime cost beyond the C calls themselves. Requires
 * C++14 for `std::exchange`, which is what examples/c/CMakeLists.txt asks for.
 */

#ifndef WICKRA_EXCHANGE_HPP
#define WICKRA_EXCHANGE_HPP

#include "wickra_exchange.h"

#include <utility>

namespace wickra {

/// Move-only RAII owner of a wickra-exchange handle. `T` is the opaque handle
/// type and `Free` its `wickra_*_free` function.
///
/// Copying is deleted rather than defaulted: two owners of one handle would free
/// it twice, and the second free is undefined behaviour rather than a diagnosed
/// error. Moving leaves the source null, so the moved-from owner frees nothing.
template <typename T, void (*Free)(T *)>
class Handle {
public:
    /// Takes ownership of `ptr`, which may be null -- a failed constructor
    /// returns null, and freeing null is defined as a no-op by the C ABI.
    explicit Handle(T *ptr) noexcept : ptr_(ptr) {}

    ~Handle() {
        if (ptr_ != nullptr) {
            Free(ptr_);
        }
    }

    Handle(const Handle &) = delete;
    Handle &operator=(const Handle &) = delete;

    Handle(Handle &&other) noexcept : ptr_(std::exchange(other.ptr_, nullptr)) {}

    Handle &operator=(Handle &&other) noexcept {
        if (this != &other) {
            if (ptr_ != nullptr) {
                Free(ptr_);
            }
            ptr_ = std::exchange(other.ptr_, nullptr);
        }
        return *this;
    }

    /// The raw handle, for passing to the `wickra_*` functions.
    T *get() const noexcept { return ptr_; }

    /// True if the handle is non-null, which is what every constructor in this
    /// ABI reports failure with.
    explicit operator bool() const noexcept { return ptr_ != nullptr; }

    /// Releases ownership without freeing, for handing the handle to code that
    /// frees it itself.
    T *release() noexcept { return std::exchange(ptr_, nullptr); }

private:
    T *ptr_;
};

/// The five handle types of the C ABI, each bound to its own free function so a
/// caller cannot pair a handle with the wrong one.
using Exchange = Handle<WickraExchange, wickra_exchange_free>;
using Derivatives = Handle<WickraDerivatives, wickra_derivatives_free>;
using Advanced = Handle<WickraAdvanced, wickra_advanced_free>;
using UserData = Handle<WickraUserData, wickra_user_data_free>;
using WsExecution = Handle<WickraWsExecution, wickra_ws_execution_free>;

}  // namespace wickra

#endif  // WICKRA_EXCHANGE_HPP
