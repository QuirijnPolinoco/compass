#include "util.h"

int main(void) {
    struct Point p = {1, 2};
    return add(p.x, p.y);
}
